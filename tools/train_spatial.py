#!/usr/bin/env python3
"""Train a spatial value net on `civvis selfplay` output.

Consumes the tensor dump written by ``civvis selfplay --out <dir>``:
a residual CNN over the map planes is fused with the global scalar vector
and trained to predict the game outcome from a fog-honest position.

    civvis selfplay --games 200 --out selfplay/run1
    python tools/train_spatial.py selfplay/run1 --epochs 40

Writes ``<dir>/spatial_value.pt`` plus ``<dir>/train_report.json``. Requires
PyTorch; uses CUDA when available (the point of the exercise on this rig).

The map wraps east-west, so horizontal convolutions use circular padding
and vertical ones zero-pad — a flat 'same' padding would teach the net that
the west edge borders empty space, which it does not.
"""
import argparse
import json
import os

import numpy as np


def load(dir_path):
    meta = json.load(open(os.path.join(dir_path, "meta.json")))
    planes = np.fromfile(os.path.join(dir_path, "planes.f32"), dtype="<f4")
    globals_ = np.fromfile(os.path.join(dir_path, "globals.f32"), dtype="<f4")
    labels = np.fromfile(os.path.join(dir_path, "labels.f32"), dtype="<f4")
    planes = planes.reshape(meta["planes_shape"])
    globals_ = globals_.reshape(meta["globals_shape"])
    labels = labels.reshape(meta["labels_shape"])
    if not len(planes):
        raise SystemExit(f"{dir_path}: no samples; run civvis selfplay first")
    return meta, planes, globals_, labels


def build(meta, channels=64, blocks=4):
    import torch
    from torch import nn

    n_planes = meta["planes_shape"][1]
    n_globals = meta["globals_shape"][1]

    class WrapConv(nn.Module):
        """3x3 convolution that wraps horizontally like the game map."""

        def __init__(self, cin, cout):
            super().__init__()
            self.conv = nn.Conv2d(cin, cout, 3, padding=0)

        def forward(self, x):
            x = torch.nn.functional.pad(x, (1, 1, 0, 0), mode="circular")
            x = torch.nn.functional.pad(x, (0, 0, 1, 1), mode="constant", value=0.0)
            return self.conv(x)

    class Block(nn.Module):
        def __init__(self, c):
            super().__init__()
            self.a, self.b = WrapConv(c, c), WrapConv(c, c)
            self.na, self.nb = nn.BatchNorm2d(c), nn.BatchNorm2d(c)

        def forward(self, x):
            y = torch.relu(self.na(self.a(x)))
            y = self.nb(self.b(y))
            return torch.relu(x + y)

    class Net(nn.Module):
        def __init__(self):
            super().__init__()
            self.stem = WrapConv(n_planes, channels)
            self.stem_norm = nn.BatchNorm2d(channels)
            self.blocks = nn.Sequential(*[Block(channels) for _ in range(blocks)])
            self.head = nn.Sequential(
                nn.Linear(channels * 2 + n_globals, 128), nn.ReLU(), nn.Linear(128, 1)
            )

        def forward(self, planes, globals_):
            x = torch.relu(self.stem_norm(self.stem(planes)))
            x = self.blocks(x)
            pooled = torch.cat(
                [x.mean(dim=(2, 3)), x.amax(dim=(2, 3)), globals_], dim=1
            )
            return self.head(pooled)

    return Net()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--epochs", type=int, default=40)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--channels", type=int, default=64)
    ap.add_argument("--blocks", type=int, default=4)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()

    try:
        import torch
        from torch import nn
    except ImportError:
        raise SystemExit("PyTorch required: pip install torch")

    meta, planes, globals_, labels = load(args.dir)
    torch.manual_seed(args.seed)
    rng = np.random.default_rng(args.seed)
    # Split BY GAME. Snapshots from one game share its outcome label and are
    # near-duplicates late on, so a per-sample split leaks the answer into
    # validation and reports an accuracy the model has not earned.
    games = labels[:, 2].astype(int)
    unique = np.unique(games)
    rng.shuffle(unique)
    held = set(unique[: max(1, len(unique) // 5)].tolist())
    val_mask = np.array([g in held for g in games])
    val_idx = np.nonzero(val_mask)[0]
    train_idx = np.nonzero(~val_mask)[0]
    print(f"{len(unique)} games -> {len(held)} held out "
          f"({len(train_idx)} train / {len(val_idx)} val samples)")

    dev = "cuda" if torch.cuda.is_available() else "cpu"
    net = build(meta, args.channels, args.blocks).to(dev)
    opt = torch.optim.Adam(net.parameters(), lr=1e-3)
    loss_fn = nn.BCEWithLogitsLoss()

    def tensors(idx):
        return (
            torch.tensor(planes[idx]).to(dev),
            torch.tensor(globals_[idx]).to(dev),
            torch.tensor(labels[idx, :1]).to(dev),
        )

    best, best_state, patience = float("inf"), None, 0
    for epoch in range(args.epochs):
        net.train()
        shuffled = rng.permutation(train_idx)
        for i in range(0, len(shuffled), args.batch):
            p, g, y = tensors(shuffled[i : i + args.batch])
            opt.zero_grad()
            loss = loss_fn(net(p, g), y)
            loss.backward()
            opt.step()
        net.eval()
        with torch.no_grad():
            vp, vg, vy = tensors(val_idx)
            logits = net(vp, vg)
            vloss = loss_fn(logits, vy).item()
            acc = (((logits > 0).float()) == vy).float().mean().item()
        print(f"epoch {epoch:3d}  val BCE {vloss:.4f}  acc {acc:.3f}")
        if vloss < best - 1e-5:
            best, patience = vloss, 0
            best_state = {k: v.detach().cpu().clone() for k, v in net.state_dict().items()}
            best_acc = acc
        else:
            patience += 1
            if patience >= 8:
                break

    # Always report the majority-class baseline. On a 4-player export only
    # one seat per game wins, so 75% accuracy is what a constant predictor
    # scores; a net that does not clearly beat this has learned nothing and
    # the run needs more games, not more epochs.
    base_rate = float(labels[train_idx, 0].mean())
    vy_np = labels[val_idx, 0]
    baseline_acc = float(max(vy_np.mean(), 1 - vy_np.mean()))
    eps = 1e-7
    p_hat = min(max(base_rate, eps), 1 - eps)
    baseline_bce = float(
        -(vy_np * np.log(p_hat) + (1 - vy_np) * np.log(1 - p_hat)).mean()
    )
    beat = best < baseline_bce - 1e-4 and best_acc > baseline_acc + 1e-4
    print(
        f"baseline (constant p={base_rate:.3f}): BCE {baseline_bce:.4f} "
        f"acc {baseline_acc:.3f}  ->  model {'BEATS' if beat else 'DOES NOT BEAT'} baseline"
    )

    out = os.path.join(args.dir, "spatial_value.pt")
    torch.save({"state_dict": best_state, "meta": meta,
                "channels": args.channels, "blocks": args.blocks}, out)
    report = {"val_bce": best, "val_acc": best_acc, "device": dev,
              "baseline_bce": baseline_bce, "baseline_acc": baseline_acc,
              "beats_baseline": bool(beat),
              "samples": int(len(planes)), "train": int(len(train_idx)),
              "val": int(len(val_idx))}
    json.dump(report, open(os.path.join(args.dir, "train_report.json"), "w"), indent=2)
    print(f"wrote {out}: val BCE {best:.4f}, acc {best_acc:.3f} on {dev}")


if __name__ == "__main__":
    main()

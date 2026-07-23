#!/usr/bin/env python3
"""Train the position value net from evolve's self-play dataset.

Reads ``evolved/dataset.csv`` (25 features + win label per row, appended by
``civvis evolve`` during SPRT confirmation games) and writes:

- ``evolved/valuenet.json`` — weights in the exact schema ``valuenet.rs``
  loads (sizes / weights[layer][in][out] / biases[layer][out]);
- ``evolved/valuenet_fixture.json`` — one held-out row with this trainer's
  output, activating the Rust/Python parity test.

Uses PyTorch with CUDA when available (the intended path on the training
rig); falls back to a pure-NumPy Adam loop so any machine can retrain.

    python tools/train_valuenet.py [--dir evolved] [--epochs 200] [--seed 7]
"""
import argparse
import csv
import json
import math
import os
import random

SIZES = [25, 64, 32, 1]


def load_rows(path):
    """Rows of (features, label, game). `civvis selfplay` appends a game
    index; `civvis evolve`'s older CSV has no game column, in which case
    every row is its own 'game' and the split degrades to per-sample."""
    rows = []
    with open(path, newline="") as f:
        for row in csv.reader(f):
            if len(row) == SIZES[0] + 2:
                game = int(float(row[-1]))
                feats, label = row[:-2], row[-2]
            elif len(row) == SIZES[0] + 1:
                game, feats, label = len(rows), row[:-1], row[-1]
            else:
                continue
            rows.append(([float(x) for x in feats], float(label), game))
    if len(rows) < 200:
        raise SystemExit(f"{path}: only {len(rows)} usable rows; run evolve longer")
    return rows


def train_torch(train, val, epochs, seed):
    import torch
    from torch import nn

    torch.manual_seed(seed)
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    net = nn.Sequential(
        nn.Linear(SIZES[0], SIZES[1]), nn.ReLU(),
        nn.Linear(SIZES[1], SIZES[2]), nn.ReLU(),
        nn.Linear(SIZES[2], SIZES[3]),
    ).to(dev)
    xt = torch.tensor([x for x, _ in train], device=dev)
    yt = torch.tensor([[y] for _, y in train], device=dev)
    xv = torch.tensor([x for x, _ in val], device=dev)
    yv = torch.tensor([[y] for _, y in val], device=dev)
    opt = torch.optim.Adam(net.parameters(), lr=1e-3)
    loss_fn = nn.BCEWithLogitsLoss()
    best, best_state, patience = float("inf"), None, 0
    for epoch in range(epochs):
        net.train()
        for i in range(0, len(xt), 512):
            opt.zero_grad()
            loss = loss_fn(net(xt[i : i + 512]), yt[i : i + 512])
            loss.backward()
            opt.step()
        net.eval()
        with torch.no_grad():
            vloss = loss_fn(net(xv), yv).item()
        if vloss < best - 1e-5:
            best, patience = vloss, 0
            best_state = [p.detach().cpu().clone() for p in net.parameters()]
        else:
            patience += 1
            if patience >= 20:
                break
    params = [p.tolist() for p in best_state]
    # torch Linear stores weight as [out][in]; the Rust net wants [in][out].
    weights = [list(map(list, zip(*params[i]))) for i in range(0, 6, 2)]
    biases = [params[i] for i in range(1, 6, 2)]
    return weights, biases, best, f"torch/{dev}"


def train_numpy(train, val, epochs, seed):
    import numpy as np

    rng = np.random.default_rng(seed)
    ws = [rng.normal(0, math.sqrt(2 / SIZES[i]), (SIZES[i], SIZES[i + 1]))
          for i in range(3)]
    bs = [np.zeros(SIZES[i + 1]) for i in range(3)]
    mw = [np.zeros_like(w) for w in ws]; vw = [np.zeros_like(w) for w in ws]
    mb = [np.zeros_like(b) for b in bs]; vb = [np.zeros_like(b) for b in bs]
    xt = np.array([x for x, _ in train]); yt = np.array([y for _, y in train])
    xv = np.array([x for x, _ in val]); yv = np.array([y for _, y in val])

    def forward(x):
        a1 = np.maximum(x @ ws[0] + bs[0], 0)
        a2 = np.maximum(a1 @ ws[1] + bs[1], 0)
        z = (a2 @ ws[2] + bs[2]).ravel()
        return a1, a2, z

    def val_loss():
        z = forward(xv)[2]
        p = 1 / (1 + np.exp(-z))
        eps = 1e-7
        return float(-np.mean(yv * np.log(p + eps) + (1 - yv) * np.log(1 - p + eps)))

    best, best_snap, patience, step = float("inf"), None, 0, 0
    for epoch in range(epochs):
        order = rng.permutation(len(xt))
        for i in range(0, len(order), 512):
            idx = order[i : i + 512]
            x, y = xt[idx], yt[idx]
            a1, a2, z = forward(x)
            p = 1 / (1 + np.exp(-z))
            dz = (p - y)[:, None] / len(idx)
            grads_w = [None] * 3; grads_b = [None] * 3
            grads_w[2] = a2.T @ dz; grads_b[2] = dz.sum(0)
            da2 = dz @ ws[2].T; da2[a2 <= 0] = 0
            grads_w[1] = a1.T @ da2; grads_b[1] = da2.sum(0)
            da1 = da2 @ ws[1].T; da1[a1 <= 0] = 0
            grads_w[0] = x.T @ da1; grads_b[0] = da1.sum(0)
            step += 1
            for j in range(3):
                for grad, param, m, v in ((grads_w[j], ws[j], mw[j], vw[j]),
                                          (grads_b[j], bs[j], mb[j], vb[j])):
                    m *= 0.9; m += 0.1 * grad
                    v *= 0.999; v += 0.001 * grad * grad
                    mh = m / (1 - 0.9 ** step)
                    vh = v / (1 - 0.999 ** step)
                    param -= 1e-3 * mh / (np.sqrt(vh) + 1e-8)
        loss = val_loss()
        if loss < best - 1e-5:
            best, patience = loss, 0
            best_snap = ([w.copy() for w in ws], [b.copy() for b in bs])
        else:
            patience += 1
            if patience >= 20:
                break
    ws, bs = best_snap
    return [w.tolist() for w in ws], [b.tolist() for b in bs], best, "numpy"


def net_eval(weights, biases, x):
    a = list(x)
    for layer, (w, b) in enumerate(zip(weights, biases)):
        nxt = list(b)
        for i, ai in enumerate(a):
            for j in range(len(nxt)):
                nxt[j] += ai * w[i][j]
        last = layer == len(weights) - 1
        a = [1 / (1 + math.exp(-v)) if last else max(v, 0.0) for v in nxt]
    return a[0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="evolved")
    ap.add_argument("--epochs", type=int, default=200)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()

    rows = load_rows(os.path.join(args.dir, "dataset.csv"))
    # Hold out whole GAMES. Snapshots from one game share its outcome label,
    # so a per-sample split leaks the answer into validation.
    games = sorted({game for _, _, game in rows})
    rng = random.Random(args.seed)
    rng.shuffle(games)
    held = set(games[: max(1, len(games) // 5)])
    val = [(f, y) for f, y, g in rows if g in held]
    train = [(f, y) for f, y, g in rows if g not in held]
    if not val or not train:
        raise SystemExit("need at least two distinct games to hold one out")
    print(f"{len(games)} games -> {len(held)} held out "
          f"({len(train)} train / {len(val)} val rows)")
    try:
        weights, biases, loss, backend = train_torch(train, val, args.epochs, args.seed)
    except ImportError:
        weights, biases, loss, backend = train_numpy(train, val, args.epochs, args.seed)

    with open(os.path.join(args.dir, "valuenet.json"), "w") as f:
        json.dump({"sizes": SIZES, "weights": weights, "biases": biases}, f)
    fixture_x = val[0][0]
    with open(os.path.join(args.dir, "valuenet_fixture.json"), "w") as f:
        json.dump({"input": fixture_x, "output": net_eval(weights, biases, fixture_x)}, f)
    wins = sum(y for _, y, _ in rows)
    # Majority-class baseline, always reported: a net that cannot beat a
    # constant predictor has learned nothing and needs more games.
    base_rate = sum(y for _, y in train) / max(1, len(train))
    vy = [y for _, y in val]
    baseline_acc = max(sum(vy) / len(vy), 1 - sum(vy) / len(vy))
    p_hat = min(max(base_rate, 1e-7), 1 - 1e-7)
    baseline_bce = -sum(
        y * math.log(p_hat) + (1 - y) * math.log(1 - p_hat) for y in vy
    ) / len(vy)
    verdict = "BEATS" if loss < baseline_bce - 1e-4 else "DOES NOT BEAT"
    print(f"trained on {len(train)} rows ({wins:.0f}/{len(rows)} wins) "
          f"via {backend}; val BCE {loss:.4f}; wrote {args.dir}/valuenet.json")
    print(f"baseline (constant p={base_rate:.3f}): BCE {baseline_bce:.4f} "
          f"acc {baseline_acc:.3f}  ->  model {verdict} baseline")


if __name__ == "__main__":
    main()

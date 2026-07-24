#!/usr/bin/env python3
"""Train the scalar position-value net on game-grouped self-play.

Input ``dataset.csv`` rows are 25 features, a win label, and a source-game
index. Whole games are split into train, early-stopping validation, and final
test sets; correlated snapshots from one game can never cross a split.

Writes ``valuenet.json``, ``valuenet_fixture.json``, and
``valuenet_metrics.json`` in ``--dir`` only when the untouched test games beat
the constant train-win-rate baseline on binary cross entropy. PyTorch is used
when available; otherwise the deterministic NumPy Adam implementation is used.

    civvis selfplay --games 300 --scalar-only --out /tmp/value-selfplay
    python tools/train_valuenet.py --dir /tmp/value-selfplay --epochs 200
"""
import argparse
import csv
import json
import math
import os
import random

SIZES = [25, 64, 32, 1]
EPSILON = 1e-7


def load_rows(path):
    """Return grouped ``(features, label, game)`` rows, failing closed on
    legacy ungrouped data rather than leaking snapshots across splits."""
    rows = []
    ungrouped = 0
    malformed = 0
    with open(path, newline="") as source:
        for raw in csv.reader(source):
            if len(raw) == SIZES[0] + 1:
                ungrouped += 1
                continue
            if len(raw) != SIZES[0] + 2:
                malformed += 1
                continue
            try:
                features = [float(value) for value in raw[:-2]]
                label = float(raw[-2])
                game_value = float(raw[-1])
            except ValueError:
                malformed += 1
                continue
            if (
                not all(math.isfinite(value) for value in features)
                or label not in (0.0, 1.0)
                or not math.isfinite(game_value)
                or not game_value.is_integer()
                or game_value < 0
            ):
                malformed += 1
                continue
            game = int(game_value)
            rows.append((features, label, game))
    if ungrouped:
        raise SystemExit(
            f"{path}: {ungrouped} rows have no source-game index; regenerate with "
            "`civvis selfplay --scalar-only` so validation cannot leak"
        )
    if malformed:
        raise SystemExit(f"{path}: {malformed} malformed rows")
    if len(rows) < 200:
        raise SystemExit(f"{path}: only {len(rows)} usable rows; run more self-play")
    return rows


def split_by_game(rows, seed):
    games = sorted({game for _, _, game in rows})
    if len(games) < 10:
        raise SystemExit("need at least 10 distinct games for train/validation/test splits")
    rng = random.Random(seed)
    rng.shuffle(games)
    test_count = max(1, len(games) // 5)
    validation_count = max(1, len(games) // 5)
    test_games = set(games[:test_count])
    validation_games = set(games[test_count : test_count + validation_count])
    train_games = set(games[test_count + validation_count :])
    split = lambda selected: [
        (features, label) for features, label, game in rows if game in selected
    ]
    train = split(train_games)
    validation = split(validation_games)
    test = split(test_games)
    if not train or not validation or not test:
        raise SystemExit("every game split must contain samples")
    return train, validation, test, train_games, validation_games, test_games


def train_torch(train, validation, epochs, seed):
    import torch
    from torch import nn

    torch.manual_seed(seed)
    device = "cuda" if torch.cuda.is_available() else "cpu"
    generator = torch.Generator(device=device)
    generator.manual_seed(seed)
    net = nn.Sequential(
        nn.Linear(SIZES[0], SIZES[1]),
        nn.ReLU(),
        nn.Linear(SIZES[1], SIZES[2]),
        nn.ReLU(),
        nn.Linear(SIZES[2], SIZES[3]),
    ).to(device)
    features = torch.tensor([x for x, _ in train], device=device)
    labels = torch.tensor([[y] for _, y in train], device=device)
    validation_features = torch.tensor([x for x, _ in validation], device=device)
    validation_labels = torch.tensor([[y] for _, y in validation], device=device)
    optimizer = torch.optim.Adam(net.parameters(), lr=1e-3)
    loss_fn = nn.BCEWithLogitsLoss()
    best = float("inf")
    best_state = None
    best_epoch = 0
    patience = 0
    for epoch in range(epochs):
        net.train()
        order = torch.randperm(len(features), generator=generator, device=device)
        for start in range(0, len(order), 512):
            indices = order[start : start + 512]
            optimizer.zero_grad()
            loss = loss_fn(net(features[indices]), labels[indices])
            loss.backward()
            optimizer.step()
        net.eval()
        with torch.no_grad():
            validation_loss = loss_fn(net(validation_features), validation_labels).item()
        if validation_loss < best - 1e-5:
            best = validation_loss
            best_epoch = epoch + 1
            patience = 0
            best_state = [parameter.detach().cpu().clone() for parameter in net.parameters()]
        else:
            patience += 1
            if patience >= 20:
                break
    params = [parameter.tolist() for parameter in best_state]
    # torch Linear stores [out][in]; Rust expects [in][out].
    weights = [list(map(list, zip(*params[index]))) for index in range(0, 6, 2)]
    biases = [params[index] for index in range(1, 6, 2)]
    return weights, biases, best, best_epoch, f"torch/{device}"


def train_numpy(train, validation, epochs, seed):
    import numpy as np

    rng = np.random.default_rng(seed)
    weights = [
        rng.normal(0, math.sqrt(2 / SIZES[index]), (SIZES[index], SIZES[index + 1]))
        for index in range(3)
    ]
    biases = [np.zeros(SIZES[index + 1]) for index in range(3)]
    mean_weights = [np.zeros_like(weight) for weight in weights]
    variance_weights = [np.zeros_like(weight) for weight in weights]
    mean_biases = [np.zeros_like(bias) for bias in biases]
    variance_biases = [np.zeros_like(bias) for bias in biases]
    features = np.array([x for x, _ in train])
    labels = np.array([y for _, y in train])
    validation_features = np.array([x for x, _ in validation])
    validation_labels = np.array([y for _, y in validation])

    def forward(batch):
        with np.errstate(over="ignore", invalid="ignore", divide="ignore"):
            hidden_one = np.maximum(batch @ weights[0] + biases[0], 0)
            hidden_two = np.maximum(hidden_one @ weights[1] + biases[1], 0)
            logits = (hidden_two @ weights[2] + biases[2]).ravel()
        if not (
            np.isfinite(hidden_one).all()
            and np.isfinite(hidden_two).all()
            and np.isfinite(logits).all()
        ):
            raise FloatingPointError("non-finite value-model activation")
        return hidden_one, hidden_two, logits

    def validation_loss():
        logits = forward(validation_features)[2]
        probabilities = 1 / (1 + np.exp(-np.clip(logits, -60, 60)))
        return float(
            -np.mean(
                validation_labels * np.log(probabilities + EPSILON)
                + (1 - validation_labels) * np.log(1 - probabilities + EPSILON)
            )
        )

    best = float("inf")
    best_snapshot = None
    best_epoch = 0
    patience = 0
    step = 0
    for epoch in range(epochs):
        order = rng.permutation(len(features))
        for start in range(0, len(order), 512):
            indices = order[start : start + 512]
            batch = features[indices]
            batch_labels = labels[indices]
            hidden_one, hidden_two, logits = forward(batch)
            probabilities = 1 / (1 + np.exp(-np.clip(logits, -60, 60)))
            logits_gradient = (probabilities - batch_labels)[:, None] / len(indices)
            gradients_weights = [None] * 3
            gradients_biases = [None] * 3
            with np.errstate(over="ignore", invalid="ignore", divide="ignore"):
                gradients_weights[2] = hidden_two.T @ logits_gradient
                gradients_biases[2] = logits_gradient.sum(0)
                hidden_two_gradient = logits_gradient @ weights[2].T
                hidden_two_gradient[hidden_two <= 0] = 0
                gradients_weights[1] = hidden_one.T @ hidden_two_gradient
                gradients_biases[1] = hidden_two_gradient.sum(0)
                hidden_one_gradient = hidden_two_gradient @ weights[1].T
                hidden_one_gradient[hidden_one <= 0] = 0
                gradients_weights[0] = batch.T @ hidden_one_gradient
                gradients_biases[0] = hidden_one_gradient.sum(0)
            if not all(
                np.isfinite(gradient).all()
                for gradient in gradients_weights + gradients_biases
            ):
                raise FloatingPointError("non-finite value-model gradient")
            for gradient in gradients_weights + gradients_biases:
                np.clip(gradient, -5.0, 5.0, out=gradient)
            step += 1
            for index in range(3):
                updates = (
                    (
                        gradients_weights[index],
                        weights[index],
                        mean_weights[index],
                        variance_weights[index],
                    ),
                    (
                        gradients_biases[index],
                        biases[index],
                        mean_biases[index],
                        variance_biases[index],
                    ),
                )
                for gradient, parameter, mean, variance in updates:
                    mean *= 0.9
                    mean += 0.1 * gradient
                    variance *= 0.999
                    variance += 0.001 * gradient * gradient
                    corrected_mean = mean / (1 - 0.9**step)
                    corrected_variance = variance / (1 - 0.999**step)
                    parameter -= 1e-3 * corrected_mean / (np.sqrt(corrected_variance) + 1e-8)
        loss = validation_loss()
        if loss < best - 1e-5:
            best = loss
            best_epoch = epoch + 1
            patience = 0
            best_snapshot = (
                [weight.copy() for weight in weights],
                [bias.copy() for bias in biases],
            )
        else:
            patience += 1
            if patience >= 20:
                break
    weights, biases = best_snapshot
    return (
        [weight.tolist() for weight in weights],
        [bias.tolist() for bias in biases],
        best,
        best_epoch,
        "numpy",
    )


def stable_sigmoid(value):
    if value >= 0:
        return 1 / (1 + math.exp(-value))
    exponential = math.exp(value)
    return exponential / (1 + exponential)


def net_eval(weights, biases, features):
    activation = list(features)
    for layer, (weight, bias) in enumerate(zip(weights, biases)):
        next_activation = list(bias)
        for input_index, input_value in enumerate(activation):
            for output_index in range(len(next_activation)):
                next_activation[output_index] += input_value * weight[input_index][output_index]
        last = layer == len(weights) - 1
        activation = [
            stable_sigmoid(value) if last else max(value, 0.0) for value in next_activation
        ]
    return activation[0]


def predict_rows(weights, biases, rows):
    try:
        import numpy as np
    except ImportError:
        return [net_eval(weights, biases, features) for features, _ in rows]
    activation = np.array([features for features, _ in rows])
    with np.errstate(over="ignore", invalid="ignore", divide="ignore"):
        for layer, (weight, bias) in enumerate(zip(weights, biases)):
            activation = activation @ np.asarray(weight) + np.asarray(bias)
            if not np.isfinite(activation).all():
                raise FloatingPointError("non-finite exported value-model prediction")
            if layer < len(weights) - 1:
                activation = np.maximum(activation, 0)
    return (1 / (1 + np.exp(-np.clip(activation.ravel(), -60, 60)))).tolist()


def probability_metrics(probabilities, labels):
    clipped = [
        min(max(probability, EPSILON), 1 - EPSILON)
        for probability in probabilities
    ]
    bce = -sum(
        label * math.log(probability) + (1 - label) * math.log(1 - probability)
        for probability, label in zip(clipped, labels)
    ) / len(labels)
    brier = sum(
        (probability - label) ** 2 for probability, label in zip(probabilities, labels)
    ) / len(labels)
    accuracy = sum(
        (probability >= 0.5) == bool(label)
        for probability, label in zip(probabilities, labels)
    ) / len(labels)
    calibration_error = 0.0
    for bin_index in range(10):
        low = bin_index / 10
        high = (bin_index + 1) / 10
        members = [
            index
            for index, probability in enumerate(probabilities)
            if low <= probability < high or (bin_index == 9 and probability == 1.0)
        ]
        if not members:
            continue
        confidence = sum(probabilities[index] for index in members) / len(members)
        observed = sum(labels[index] for index in members) / len(members)
        calibration_error += len(members) / len(labels) * abs(confidence - observed)
    return {
        "bce": bce,
        "brier": brier,
        "accuracy": accuracy,
        "ece_10": calibration_error,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", default="evolved")
    parser.add_argument("--epochs", type=int, default=200)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument(
        "--allow-nonimproving",
        action="store_true",
        help="write artifacts even when final-test BCE does not beat the constant baseline",
    )
    args = parser.parse_args()
    if args.epochs < 1:
        parser.error("--epochs must be at least 1")

    rows = load_rows(os.path.join(args.dir, "dataset.csv"))
    train, validation, test, train_games, validation_games, test_games = split_by_game(
        rows, args.seed
    )
    print(
        f"{len(train_games)}/{len(validation_games)}/{len(test_games)} "
        f"train/validation/test games -> {len(train)}/{len(validation)}/{len(test)} rows"
    )
    try:
        weights, biases, validation_bce, best_epoch, backend = train_torch(
            train, validation, args.epochs, args.seed
        )
    except ImportError:
        weights, biases, validation_bce, best_epoch, backend = train_numpy(
            train, validation, args.epochs, args.seed
        )

    test_labels = [label for _, label in test]
    test_predictions = predict_rows(weights, biases, test)
    model_metrics = probability_metrics(test_predictions, test_labels)
    train_rate = sum(label for _, label in train) / len(train)
    baseline_metrics = probability_metrics([train_rate] * len(test), test_labels)
    improves = model_metrics["bce"] < baseline_metrics["bce"] - 1e-4
    verdict = "BEATS" if improves else "DOES NOT BEAT"
    print(
        f"best epoch {best_epoch} via {backend}; validation BCE {validation_bce:.4f}; "
        f"test BCE {model_metrics['bce']:.4f}, Brier {model_metrics['brier']:.4f}, "
        f"ECE {model_metrics['ece_10']:.4f}"
    )
    print(
        f"constant p={train_rate:.4f}: test BCE {baseline_metrics['bce']:.4f}, "
        f"Brier {baseline_metrics['brier']:.4f} -> model {verdict} baseline"
    )
    test_by_turn = {}
    for name, low, high in (
        ("opening", 0.0, 0.25),
        ("early_midgame", 0.25, 0.5),
        ("late_midgame", 0.5, 0.75),
        ("endgame", 0.75, 1.01),
    ):
        indices = [
            index
            for index, (features, _) in enumerate(test)
            if low <= features[-1] < high
        ]
        if not indices:
            continue
        labels = [test_labels[index] for index in indices]
        predictions = [test_predictions[index] for index in indices]
        bucket_model = probability_metrics(predictions, labels)
        bucket_baseline = probability_metrics([train_rate] * len(indices), labels)
        test_by_turn[name] = {
            "rows": len(indices),
            "model": bucket_model,
            "constant_baseline": bucket_baseline,
        }
        print(
            f"  {name:<13} rows={len(indices):4} model BCE {bucket_model['bce']:.4f} "
            f"vs constant {bucket_baseline['bce']:.4f}"
        )
    if not improves and not args.allow_nonimproving:
        raise SystemExit("refusing to write a value model that fails unseen-game BCE")

    model = {"sizes": SIZES, "weights": weights, "biases": biases}
    fixture_features = test[0][0]
    metrics = {
        "seed": args.seed,
        "backend": backend,
        "best_epoch": best_epoch,
        "games": {
            "train": len(train_games),
            "validation": len(validation_games),
            "test": len(test_games),
        },
        "rows": {"train": len(train), "validation": len(validation), "test": len(test)},
        "train_win_rate": train_rate,
        "validation_bce": validation_bce,
        "test_model": model_metrics,
        "test_constant_baseline": baseline_metrics,
        "test_by_turn": test_by_turn,
        "beats_baseline": improves,
    }
    with open(os.path.join(args.dir, "valuenet.json"), "w") as output:
        json.dump(model, output)
    with open(os.path.join(args.dir, "valuenet_fixture.json"), "w") as output:
        json.dump(
            {"input": fixture_features, "output": net_eval(weights, biases, fixture_features)},
            output,
        )
    with open(os.path.join(args.dir, "valuenet_metrics.json"), "w") as output:
        json.dump(metrics, output, indent=2)
    print(f"wrote grouped-test artifacts to {args.dir}")


if __name__ == "__main__":
    main()

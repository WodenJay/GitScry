#!/usr/bin/env python3
"""Disposable historical-replay probe for GitScry issue 4.

Input is `git log --diff-merges=first-parent --format='@@@%H%x09%aI%x09%s' --name-only`.
"""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
from pathlib import PurePosixPath


@dataclass
class Commit:
    sha: str
    date: str
    subject: str
    paths: tuple[str, ...]


def load(path: str) -> list[Commit]:
    commits: list[Commit] = []
    header: tuple[str, str, str] | None = None
    paths: list[str] = []
    with open(path, encoding="utf-8") as source:
        for raw in source:
            line = raw.rstrip("\n")
            if line.startswith("@@@"):
                if header:
                    commits.append(Commit(*header, tuple(dict.fromkeys(paths))))
                sha, date, subject = line[3:].split("\t", 2)
                header, paths = (sha, date, subject), []
            elif line:
                paths.append(line)
    if header:
        commits.append(Commit(*header, tuple(dict.fromkeys(paths))))
    return commits  # newest first


def is_test(path: str) -> bool:
    parts = PurePosixPath(path).parts
    name = parts[-1].lower() if parts else ""
    return "tests" in parts or "test" in parts or name.startswith("test_") or ".test." in name


def train(commits: list[Commit], target: str, limit: int) -> tuple[Commit, Counter[str], Counter[tuple[str, str]]]:
    index = next(i for i, commit in enumerate(commits) if commit.sha.startswith(target))
    target_commit = commits[index]
    older = commits[index + 1 : index + 1 + limit]
    touches: Counter[str] = Counter()
    pairs: Counter[tuple[str, str]] = Counter()
    for commit in older:
        paths = sorted(set(commit.paths))
        if not 1 < len(paths) <= 50:
            continue
        touches.update(paths)
        for left_index, left in enumerate(paths):
            for right in paths[left_index + 1 :]:
                pairs[left, right] += 1
    return target_commit, touches, pairs


def related(seed: str, touches: Counter[str], pairs: Counter[tuple[str, str]], tests_only: bool = False):
    rows = []
    for (left, right), support in pairs.items():
        if seed == left:
            candidate = right
        elif seed == right:
            candidate = left
        else:
            continue
        if tests_only and not is_test(candidate):
            continue
        confidence = support / touches[seed]
        # Support discounts one-off coincidences; confidence penalizes ubiquitous seeds.
        score = support * confidence
        rows.append((score, support, confidence, touches[candidate], candidate))
    return sorted(rows, reverse=True)


def command_case(args: argparse.Namespace) -> None:
    target, touches, pairs = train(load(args.log), args.commit, args.train)
    print(f"{target.sha} {target.date} {target.subject}")
    print("changed:")
    for path in target.paths:
        print(f"  {path}")
    for seed in args.seed or target.paths:
        print(f"\nseed: {seed} (prior touches: {touches[seed]})")
        for label, tests_only in (("related", False), ("tests", True)):
            print(f"  {label}:")
            for score, support, confidence, candidate_touches, candidate in related(seed, touches, pairs, tests_only)[: args.top]:
                hit = "*" if candidate in target.paths else " "
                print(f"   {hit} {score:7.2f} n={support:<3} p={confidence:5.1%} seen={candidate_touches:<3} {candidate}")


def command_hotspots(args: argparse.Namespace) -> None:
    target, touches, _ = train(load(args.log), args.commit, args.train)
    ranked = touches.most_common()
    ranks = {path: index + 1 for index, (path, _) in enumerate(ranked)}
    print(f"{target.sha} {target.date} {target.subject}")
    print("changed path rank among prior-history touch counts:")
    for path in target.paths:
        print(f"  rank={ranks.get(path, '-'):>6} touches={touches[path]:>4} {path}")
    print("top hotspots:")
    for path, count in ranked[: args.top]:
        print(f"  {count:>4} {path}")


def directory(path: str, depth: int) -> str:
    parts = PurePosixPath(path).parts
    return "/".join(parts[:depth]) if len(parts) >= depth else path


def command_clusters(args: argparse.Namespace) -> None:
    commits = load(args.log)
    if args.before:
        index = next(i for i, commit in enumerate(commits) if commit.sha.startswith(args.before))
        commits = commits[index + 1 :]
    commits = commits[: args.train]
    touches: Counter[str] = Counter()
    pairs: Counter[tuple[str, str]] = Counter()
    for commit in commits:
        groups = sorted({directory(path, args.depth) for path in commit.paths})
        if not 1 < len(groups) <= 20:
            continue
        touches.update(groups)
        for index, left in enumerate(groups):
            for right in groups[index + 1 :]:
                pairs[left, right] += 1
    rows = []
    for (left, right), support in pairs.items():
        if left == right or support < args.support:
            continue
        confidence = min(support / touches[left], support / touches[right])
        rows.append((support * confidence, support, confidence, left, right))
    for score, support, confidence, left, right in sorted(rows, reverse=True)[: args.top]:
        print(f"{score:7.2f} n={support:<4} min-p={confidence:5.1%} {left} <-> {right}")


def self_check() -> None:
    touches = Counter({"a": 4, "b": 3, "tests/test_a.py": 2})
    pairs = Counter({("a", "b"): 3, ("a", "tests/test_a.py"): 2})
    assert related("a", touches, pairs)[0][-1] == "b"
    assert related("a", touches, pairs, tests_only=True)[0][-1] == "tests/test_a.py"
    assert is_test("src/widget.test.ts") and not is_test("src/widget.py")
    print("ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    case = subparsers.add_parser("case")
    case.add_argument("log")
    case.add_argument("commit")
    case.add_argument("--seed", action="append")
    case.add_argument("--train", type=int, default=4000)
    case.add_argument("--top", type=int, default=10)
    case.set_defaults(run=command_case)

    hotspots = subparsers.add_parser("hotspots")
    hotspots.add_argument("log")
    hotspots.add_argument("commit")
    hotspots.add_argument("--train", type=int, default=4000)
    hotspots.add_argument("--top", type=int, default=20)
    hotspots.set_defaults(run=command_hotspots)

    clusters = subparsers.add_parser("clusters")
    clusters.add_argument("log")
    clusters.add_argument("--before")
    clusters.add_argument("--train", type=int, default=4000)
    clusters.add_argument("--depth", type=int, default=2)
    clusters.add_argument("--support", type=int, default=3)
    clusters.add_argument("--top", type=int, default=30)
    clusters.set_defaults(run=command_clusters)

    check = subparsers.add_parser("self-check")
    check.set_defaults(run=lambda _: self_check())

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Power estimate for the two-arm review-precision experiment (dev-only).

Question this answers BEFORE any code is written: how many commits must the
experiment cover to detect a plausible improvement in the invalid share of a
reviewer's findings?

Design being powered: each commit is reviewed twice (blind arm A,
contextualized arm B). Every finding is adjudicated valid or invalid. The
primary metric is the difference in invalid PROPORTION between arms. The unit
of analysis is the FINDING, not the commit, so total N is
(commits x findings per commit x 2 arms).

Baseline: 56.3% of agentic review comments are rejected (arXiv:2607.03316).
We treat that as arm A's invalid share, p_a = 0.563.

Method: normal approximation for the difference of two independent
proportions, two-sided, alpha = 0.05. n per arm for power (1-beta):

    n = (z_{1-alpha/2} + z_{1-beta})^2 * (p_a(1-p_a) + p_b(1-p_b)) / (p_a-p_b)^2

Stdlib only, no scipy: the two z values needed are hardcoded constants, which
is honest because alpha and power are fixed by the design and not swept.
"""

from __future__ import annotations

# Two-sided alpha = 0.05, and power = 0.80. Fixed by the design, not swept,
# so hardcoding them is a statement of the design rather than a shortcut.
Z_ALPHA_2 = 1.959963985
Z_POWER = 0.8416212336

P_A = 0.563  # arXiv:2607.03316 rejection rate, arm A's assumed invalid share


def n_per_arm(p_a: float, p_b: float) -> float:
    """Findings needed PER ARM to detect p_a - p_b at alpha=.05, power=.80."""
    if p_a == p_b:
        return float("inf")
    num = (Z_ALPHA_2 + Z_POWER) ** 2 * (p_a * (1 - p_a) + p_b * (1 - p_b))
    return num / (p_a - p_b) ** 2


def main() -> None:
    print(f"Baseline invalid share (arm A): {P_A:.3f}  [arXiv:2607.03316]")
    print("alpha=0.05 two-sided, power=0.80, unit of analysis = one finding\n")
    print(f"{'improvement':>12} {'arm B share':>12} {'findings/arm':>13} "
          f"{'commits @3':>11} {'commits @6':>11}")
    for delta in (0.05, 0.10, 0.15, 0.20, 0.25):
        p_b = P_A - delta
        n = n_per_arm(P_A, p_b)
        # Commits needed, assuming a typical review yields 3 or 6 findings.
        print(f"{delta:>11.0%} {p_b:>12.3f} {n:>13.0f} "
              f"{n / 3:>11.0f} {n / 6:>11.0f}")
    print("\nGO/NO-GO: compare the rightmost columns against the number of")
    print("commits you can realistically review TWICE. If the smallest")
    print("improvement worth caring about needs more commits than you can")
    print("run, the finding-level design cannot answer the question and the")
    print("experiment must be redesigned (e.g. paired per-finding adjudication")
    print("on the SAME findings, which is far more efficient) BEFORE coding.")


if __name__ == "__main__":
    main()

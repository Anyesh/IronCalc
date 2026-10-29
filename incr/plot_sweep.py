"""Draws incr/sweep-<env>.svg from incr/data/sweep-<env>.csv.

uv run --with matplotlib python incr/plot_sweep.py native node
"""

import csv
import sys
from collections import defaultdict
from pathlib import Path

import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter

HERE = Path(__file__).parent
SURFACE = "#fcfcfb"
INK = "#0b0b0b"
MUTED = "#52514e"
GRID = "#e4e3df"
INCR = "#2a78d6"
SHIPPED = "#eb6834"
PANELS = [
    ("chains edit one head", "Edit one value in a chain"),
    ("running sum edit the last row", "Edit the last row"),
    ("running sum edit the midpoint", "Edit the middle row"),
    ("running sum edit A1", "Edit A1 (every cell changes)"),
    ("first evaluate", "Open the file (first calculation)"),
]
TITLES = {"native": "Native (Rust)", "node": "Browser engine (wasm on Node)"}


def read(env):
    series = defaultdict(list)
    with open(HERE / "data" / f"sweep-{env}.csv") as f:
        for row in csv.DictReader(f):
            series[row["workload"]].append(
                (int(row["rows"]), float(row["incr_ms"]), float(row["reference_ms"]))
            )
    return series


def duration(ms, _pos=None):
    if ms >= 1000:
        return f"{ms / 1000:g} s"
    if ms >= 1:
        return f"{ms:g} ms"
    return f"{ms * 1000:g} µs"


def rows_label(rows, _pos=None):
    return f"{rows / 1e6:g}M" if rows >= 1e6 else f"{rows / 1e3:g}k"


def plot(env):
    series = read(env)
    fig, axes = plt.subplots(1, len(PANELS), figsize=(16, 3.8), sharey=True)
    fig.patch.set_facecolor(SURFACE)
    for ax, (key, title) in zip(axes, PANELS):
        points = sorted(series[key])
        rows = [p[0] for p in points]
        ax.set_facecolor(SURFACE)
        ax.plot(rows, [p[2] for p in points], color=SHIPPED, lw=2, marker="o", ms=5)
        ax.plot(rows, [p[1] for p in points], color=INCR, lw=2, marker="o", ms=5)
        ax.set_xscale("log")
        ax.set_yscale("log")
        ax.set_title(title, color=INK, fontsize=11, loc="left")
        ax.xaxis.set_major_formatter(FuncFormatter(rows_label))
        ax.xaxis.set_minor_formatter(FuncFormatter(lambda *_: ""))
        ax.yaxis.set_major_formatter(FuncFormatter(duration))
        ax.set_xticks(rows)
        ax.tick_params(colors=MUTED, labelsize=9)
        ax.grid(True, which="major", color=GRID, lw=0.8)
        for side in ("top", "right"):
            ax.spines[side].set_visible(False)
        for side in ("left", "bottom"):
            ax.spines[side].set_color(GRID)
        ax.set_xlabel("formulas in the sheet", color=MUTED, fontsize=9)
    axes[0].set_ylabel("time (log scale)", color=MUTED, fontsize=9)
    handles = [
        plt.Line2D([], [], color=SHIPPED, lw=2, marker="o", label="IronCalc today"),
        plt.Line2D([], [], color=INCR, lw=2, marker="o", label="IronCalc with incr"),
    ]
    fig.legend(
        handles=handles,
        loc="upper right",
        ncol=2,
        frameon=False,
        labelcolor=INK,
        fontsize=10,
    )
    fig.suptitle(TITLES[env], x=0.01, ha="left", color=INK, fontsize=13)
    fig.tight_layout(rect=(0, 0, 1, 0.93))
    fig.savefig(HERE / f"sweep-{env}.svg", facecolor=SURFACE)


if __name__ == "__main__":
    for env in sys.argv[1:] or ["native", "node"]:
        plot(env)

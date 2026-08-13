from pathlib import Path

import matplotlib.pyplot as plt
from matplotlib.patches import Ellipse, FancyBboxPatch, Rectangle


OUT = Path(__file__).with_name("soulseek-bot-ecosystem.png")

PAPER = "#F3E7D1"
PAPER_DARK = "#E6D3B4"
CREAM = "#FFF8EA"
ESPRESSO = "#3A271F"
MUTED = "#776458"
TERRACOTTA = "#B95F43"
SAGE = "#68775D"
MUSTARD = "#C69137"
LATTE = "#C69A6B"
REST = "#D8C5A6"

totals = {
    "accounts": 580023,
    "searches": 112005939,
    "download_files": 410460,
    "download_bytes": 9140442618460,
}

bots = {
    "accounts": 7247,
    "searches": 11749358,
    "download_files": 70219,
    "download_bytes": 1763267296082,
}

families = [
    {
        "name": "SLSK CLOUD",
        "accounts": "46 rotating IDs",
        "pattern": "slsk_<12> / <hex16>",
        "network": "DigitalOcean + Contabo\n159.203.85 · 213.199.47",
        "action": "1.41M searches",
        "color": ESPRESSO,
    },
    {
        "name": "AU ID ROTATION",
        "accounts": "3,747 rotating IDs",
        "pattern": "ms22… + random16",
        "network": "43 IPs · residential ↔ M247\n193.56.253 · 217.138.205 · 93.115.35",
        "action": "908K searches",
        "color": TERRACOTTA,
    },
    {
        "name": "XYZ CHURN",
        "accounts": "2,806 rotating IDs",
        "pattern": "[x, y, z] × 10",
        "network": "one resolved IP\n74.101.2/24",
        "action": "85K searches",
        "color": MUSTARD,
    },
    {
        "name": "OPTBATCH HPC",
        "accounts": "240 rotating IDs",
        "pattern": "optbatch_hpc_###_<hex>",
        "network": "69 VPN / cloud IPs\nDatacamp + Clouvider",
        "action": "102K searches",
        "color": LATTE,
    },
    {
        "name": "MICRO ROTATIONS",
        "accounts": "38 rotating IDs",
        "pattern": "flem… / bbdakota…",
        "network": "2 exact IPs\nnear-identical aliases",
        "action": "303K searches",
        "color": SAGE,
    },
    {
        "name": "TEST + PROBE",
        "accounts": "156 rotating IDs",
        "pattern": "seeleseek / nic24 / test",
        "network": "6 resolved IPs\nshort-lived test cycles",
        "action": "4K searches",
        "color": MUSTARD,
    },
]

specials = [
    ("SEARCH SWEEPERS", "197 accounts", "8.81M", "searches", TERRACOTTA),
    ("QUEUE RETRY BOTS", "10 accounts", "1.74 TB", "68.6K files", SAGE),
    ("CATALOG CRAWLERS", "7 accounts", "471K", "folder requests", MUSTARD),
]


def text(fig, x, y, value, size, color=ESPRESSO, weight="normal", ha="left", va="top", family="DejaVu Sans"):
    return fig.text(
        x,
        y,
        value,
        fontsize=size,
        color=color,
        fontweight=weight,
        ha=ha,
        va=va,
        fontfamily=family,
    )


def round_box(fig, x, y, w, h, color, edge=None, radius=0.015, width=1.2):
    patch = FancyBboxPatch(
        (x, y),
        w,
        h,
        transform=fig.transFigure,
        boxstyle=f"round,pad=0.006,rounding_size={radius}",
        facecolor=color,
        edgecolor=edge or color,
        linewidth=width,
    )
    fig.add_artist(patch)
    return patch


def bean(fig, x, y, angle):
    fig.add_artist(
        Ellipse(
            (x, y),
            0.018,
            0.034,
            angle=angle,
            transform=fig.transFigure,
            facecolor=ESPRESSO,
            edgecolor="none",
        )
    )


fig = plt.figure(figsize=(12, 15), dpi=150, facecolor=PAPER)
fig.add_artist(Rectangle((0.028, 0.022), 0.944, 0.956, transform=fig.transFigure, fill=False, edgecolor=ESPRESSO, linewidth=2.2))
fig.add_artist(Rectangle((0.036, 0.030), 0.928, 0.940, transform=fig.transFigure, fill=False, edgecolor=LATTE, linewidth=1.0))

bean(fig, 0.915, 0.935, 28)
bean(fig, 0.939, 0.928, -22)

text(fig, 0.065, 0.952, "SOULSEEK  ·  30-DAY HOUSE REPORT", 10, TERRACOTTA, "bold")
text(fig, 0.065, 0.917, "The Bot Café", 37, ESPRESSO, "bold", family="DejaVu Serif")
text(fig, 0.065, 0.870, "High-confidence automation seen by one observer.", 13, MUTED)
text(fig, 0.935, 0.952, "11 JUL — 10 AUG 2026", 9, MUTED, "bold", "right")

round_box(fig, 0.065, 0.807, 0.87, 0.045, PAPER_DARK, edge=LATTE, radius=0.011)
text(fig, 0.085, 0.836, "7,247", 18, ESPRESSO, "bold")
text(fig, 0.177, 0.831, "identified bot accounts", 9.5, MUTED, "bold")
text(fig, 0.465, 0.836, "580,023", 18, ESPRESSO, "bold")
text(fig, 0.568, 0.831, "usernames observed", 9.5, MUTED, "bold")
text(fig, 0.915, 0.831, "29.8 days", 10, TERRACOTTA, "bold", "right")

text(fig, 0.065, 0.783, "TODAY’S SPLIT", 10, MUTED, "bold")

search_share = bots["searches"] / totals["searches"]
download_share = bots["download_bytes"] / totals["download_bytes"]

pie_specs = [
    (0.07, "SEARCH TRAFFIC", search_share, TERRACOTTA, "11.75M of 112.01M searches"),
    (0.55, "DOWNLOAD TRAFFIC", download_share, SAGE, "1.76 TB of 9.14 TB · 17.1% of files"),
]

for x, title, share, color, note in pie_specs:
    ax = fig.add_axes([x, 0.594, 0.34, 0.185], facecolor=PAPER)
    ax.pie(
        [share, 1 - share],
        startangle=90,
        counterclock=False,
        colors=[color, REST],
        wedgeprops={"edgecolor": PAPER, "linewidth": 4, "width": 0.33},
    )
    ax.set_aspect("equal")
    ax.text(0, 0.05, f"{share:.1%}", ha="center", va="center", color=ESPRESSO, fontsize=22, fontweight="bold")
    ax.text(0, -0.20, "BOT", ha="center", va="center", color=color, fontsize=9, fontweight="bold")
    text(fig, x + 0.34, 0.750, title, 12, ESPRESSO, "bold", "right")
    text(fig, x + 0.34, 0.618, note, 9.5, MUTED, "bold", "right")

text(fig, 0.065, 0.580, "MEET THE REGULARS", 10, MUTED, "bold")
text(fig, 0.065, 0.557, "Six fleets, six calling cards", 20, ESPRESSO, "bold", family="DejaVu Serif")

header_y = 0.525
text(fig, 0.075, header_y, "FAMILY", 8.2, MUTED, "bold")
text(fig, 0.280, header_y, "NAME FORMAT", 8.2, MUTED, "bold")
text(fig, 0.492, header_y, "NETWORK", 8.2, MUTED, "bold")
text(fig, 0.915, header_y, "ACTIVITY", 8.2, MUTED, "bold", "right")

row_top = 0.502
row_h = 0.051
for index, family in enumerate(families):
    y = row_top - index * row_h
    if index % 2 == 0:
        round_box(fig, 0.062, y - 0.039, 0.876, 0.046, CREAM, edge=CREAM, radius=0.008)
    fig.add_artist(Rectangle((0.073, y - 0.033), 0.006, 0.031, transform=fig.transFigure, facecolor=family["color"], edgecolor="none"))
    text(fig, 0.088, y, family["name"], 9.7, family["color"], "bold")
    text(fig, 0.088, y - 0.018, family["accounts"], 7.8, MUTED, "bold")
    text(fig, 0.280, y, family["pattern"], 8.0, ESPRESSO, "bold", family="DejaVu Sans Mono")
    text(fig, 0.492, y, family["network"], 8.0, MUTED, "normal")
    text(fig, 0.915, y, family["action"], 9.4, ESPRESSO, "bold", "right")

text(fig, 0.065, 0.180, "WHAT THEY’RE DOING", 10, MUTED, "bold")
text(fig, 0.065, 0.157, "The loudest jobs on the network", 19, ESPRESSO, "bold", family="DejaVu Serif")

special_y = 0.055
special_gap = 0.018
special_w = (0.87 - 2 * special_gap) / 3
for index, (name, accounts, pct, note, color) in enumerate(specials):
    x = 0.065 + index * (special_w + special_gap)
    round_box(fig, x, special_y, special_w, 0.070, PAPER_DARK, edge=LATTE, radius=0.012)
    text(fig, x + 0.014, special_y + 0.054, name, 9.2, color, "bold")
    text(fig, x + 0.014, special_y + 0.035, accounts, 8.3, MUTED, "bold")
    text(fig, x + special_w - 0.014, special_y + 0.050, pct, 19, ESPRESSO, "bold", "right")
    text(fig, x + special_w - 0.014, special_y + 0.023, note, 8.2, MUTED, "bold", "right")

fig.savefig(OUT, dpi=150, facecolor=PAPER)
plt.close(fig)
print(OUT)

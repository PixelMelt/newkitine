import subprocess
from pathlib import Path

BASE, MANTLE, SURFACE, TEXT, SUBTEXT = "#1e1e2e", "#181825", "#313244", "#cdd6f4", "#a6adc8"
COLORS = {
    "Backend": "#89b4fa",
    "Tests": "#94e2d5",
    "UI code": "#cba6f7",
    "UI styles": "#fab387",
    "Other": "#f9e2af",
}
FONT = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
WIDTH, PAD, ROW = 760, 24, 26


def lines(text):
    return sum(1 for line in text.splitlines() if line.strip())


def classify(path, text):
    if path.startswith("src/") and path.endswith(".rs"):
        code, marker, tests = text.partition("#[cfg(test)]")
        yield "Backend", None, lines(code)
        yield "Tests", None, lines(marker + tests)
    elif path.startswith("web/src/") and path.endswith(".svelte"):
        yield "UI code", "components", lines(text)
    elif path.startswith("web/src/") and path.endswith(".js"):
        yield "UI code", "logic", lines(text)
    elif path.startswith("web/") and path.endswith(".css"):
        yield "UI styles", None, lines(text)
    elif path.startswith("docs/") and path.endswith(".md"):
        yield "Other", "decision records", lines(text)


def count():
    parts = {name: {} for name in COLORS}
    for path in subprocess.check_output(["git", "ls-files"], text=True).splitlines():
        text = Path(path).read_text(errors="replace") if Path(path).is_file() else ""
        for part, sub, n in classify(path, text):
            parts[part][sub] = parts[part].get(sub, 0) + n
    return parts


def render(parts):
    totals = {name: sum(subs.values()) for name, subs in parts.items()}
    total = sum(totals.values())
    inner = WIDTH - 2 * PAD
    out = []

    def text(x, y, value, fill=TEXT, size=13, weight=400, anchor="start"):
        out.append(
            f'<text x="{x}" y="{y}" fill="{fill}" font-size="{size}" font-weight="{weight}" '
            f'text-anchor="{anchor}">{value}</text>'
        )

    text(PAD, 40, "Newkitine line count", size=17, weight=600)
    text(WIDTH - PAD, 40, f"{total:,}", size=22, weight=600, anchor="end")
    out.append(f'<rect x="{PAD}" y="58" width="{inner}" height="26" rx="4" fill="{MANTLE}"/>')
    x = PAD
    for name, n in totals.items():
        width = inner * n / total
        out.append(f'<rect x="{x:.1f}" y="58" width="{width:.1f}" height="26" fill="{COLORS[name]}"/>')
        x += width

    y = 112
    for name, subs in parts.items():
        out.append(f'<line x1="{PAD}" y1="{y - 17}" x2="{WIDTH - PAD}" y2="{y - 17}" stroke="{SURFACE}"/>')
        out.append(f'<rect x="{PAD}" y="{y - 10}" width="10" height="10" rx="2" fill="{COLORS[name]}"/>')
        text(PAD + 19, y, name, weight=600)
        text(WIDTH - PAD - 90, y, f"{totals[name]:,}", weight=600, anchor="end")
        text(WIDTH - PAD, y, f"{totals[name] / total:.1%}", weight=600, anchor="end")
        y += ROW
        named = sorted((n, sub) for sub, n in subs.items() if sub)
        for n, sub in reversed(named if len(named) > 1 else []):
            text(PAD + 19, y, sub, fill=SUBTEXT)
            text(WIDTH - PAD - 90, y, f"{n:,}", fill=SUBTEXT, anchor="end")
            text(WIDTH - PAD, y, f"{n / total:.1%}", fill=SUBTEXT, anchor="end")
            y += ROW

    height = y - ROW + 22
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {height}" width="{WIDTH}" '
        f'font-family="{FONT}">'
        f'<rect width="{WIDTH}" height="{height}" rx="8" fill="{BASE}"/>' + "".join(out) + "</svg>\n"
    )


Path("resources/loc.svg").write_text(render(count()))

#!/usr/bin/env python3
"""Consistency check and status summary for docs/FEATURES.md and docs/BACKLOG.md.

    python tools/docs_check.py            # report problems + summary
    python tools/docs_check.py --write    # also rewrite the summary block of FEATURES.md

Checks: every feature points to existing backlog items (and every non-`fora` feature has one), every
backlog item points to existing features and dependencies, items started before their dependencies are
done (the "foundation first" rule), IDs are unique.
"""
import re
import sys
from collections import Counter, OrderedDict
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent / "docs"
FEAT = DOCS / "FEATURES.md"
BACK = DOCS / "BACKLOG.md"

AREAS = OrderedDict([
    ("PRJ", "Projeto e arquivos"), ("TXS", "Texture sets e canais"), ("LYR", "Layer stack"),
    ("BLD", "Blend modes"), ("EFX", "Efeitos"), ("PRO", "Projeções"), ("TLS", "Ferramentas de pintura"),
    ("BRS", "Pincéis, alphas, stencils"), ("SUB", "Procedurais"), ("RES", "Recursos e shelf"),
    ("BAK", "Bake"), ("VPT", "Viewport e câmera"), ("RND", "Shaders e render"), ("CLR", "Cor"),
    ("EXP", "Exportação"), ("UI", "Interface"), ("SCR", "Scripting"), ("PRF", "Performance"),
    ("PLS", "ShardStroke+"),
])
STATUS = ["feito", "parcial", "aprox", "leitura", "pendente", "redesenhar", "fora"]
ID_RE = re.compile(r"^[A-Z]{2,3}-\d{2}$")
B_RE = re.compile(r"B-\d{3}")
RANGE_RE = re.compile(r"([A-Z]{2,3})-(\d{2})\.\.(\d{2})")
FID_RE = re.compile(r"[A-Z]{2,3}-\d{2}")


def cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def tables(path):
    """Yield (header, rows) for every markdown table whose first column is `ID`."""
    header = None
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.startswith("|"):
            header = None
            continue
        c = cells(line)
        if c and c[0] == "ID":
            header = c
            continue
        if header and set("".join(c)) <= set("-: "):
            continue
        if header:
            yield header, c


def expand(text):
    ids = []
    for m in RANGE_RE.finditer(text):
        a, b = int(m.group(2)), int(m.group(3))
        ids += [f"{m.group(1)}-{n:02d}" for n in range(a, b + 1)]
    text = RANGE_RE.sub("", text)
    ids += FID_RE.findall(text)
    return ids


def load_features():
    feats = OrderedDict()
    for header, row in tables(FEAT):
        if not ID_RE.match(row[0]) or len(row) != len(header):
            continue
        d = dict(zip(header, row))
        if row[0] in feats:
            print(f"ERRO: ID de feature duplicado {row[0]}")
        feats[row[0]] = {
            "status": d.get("Status", "").strip("` "),
            "ent": d.get("Ent.", ""),
            "backlog": B_RE.findall(d.get("Backlog", "")),
        }
    return feats


def load_backlog():
    items = OrderedDict()
    for header, row in tables(BACK):
        if not re.match(r"^B-\d{3}$", row[0]) or len(row) != len(header):
            continue
        d = dict(zip(header, row))
        if row[0] in items:
            print(f"ERRO: ID de backlog duplicado {row[0]}")
        items[row[0]] = {
            "status": d["Status"],
            "deps": B_RE.findall(d.get("Depende", "")),
            "features": expand(d.get("Features", "")),
            "prio": d.get("Prio", ""),
        }
    return items


def summary(feats):
    lines = ["<!-- resumo:inicio -->", "## Resumo", "",
             "| Área | Total | " + " | ".join(STATUS) + " |", "|---|---|" + "---|" * len(STATUS)]
    tot = Counter()
    for pre, name in AREAS.items():
        c = Counter(f["status"] for k, f in feats.items() if k.startswith(pre + "-"))
        n = sum(c.values())
        if not n:
            continue
        tot.update(c)
        lines.append(f"| {pre}. {name} | {n} | " + " | ".join(str(c[s]) if c[s] else "" for s in STATUS) + " |")
    n = sum(tot.values())
    lines.append(f"| **Total** | **{n}** | " + " | ".join(f"**{tot[s]}**" for s in STATUS) + " |")
    done = tot["feito"] + tot["parcial"] + tot["aprox"] + tot["leitura"]
    lines += ["", f"Cobertura (feito, parcial, aprox ou leitura): {done}/{n - tot['fora']} "
              f"({100 * done / max(1, n - tot['fora']):.0f}%). Só `feito`: {tot['feito']}/{n - tot['fora']}.",
              "", "<!-- resumo:fim -->"]
    return "\n".join(lines)


def main():
    write = "--write" in sys.argv
    feats, items = load_features(), load_backlog()
    problems = 0

    def err(msg):
        nonlocal problems
        problems += 1
        print("PROBLEMA:", msg)

    for k, f in feats.items():
        if f["status"] not in STATUS:
            err(f"{k}: status inválido '{f['status']}'")
        if f["status"] != "fora" and not f["backlog"]:
            err(f"{k}: sem item de backlog")
        for b in f["backlog"]:
            if b not in items:
                err(f"{k}: backlog {b} não existe")
    for b, it in items.items():
        for fid in it["features"]:
            if fid not in feats:
                err(f"{b}: feature {fid} não existe")
        for d in it["deps"]:
            if d not in items:
                err(f"{b}: dependência {d} não existe")
            elif it["status"] in ("doing", "done") and items[d]["status"] != "done":
                err(f"{b} ({it['status']}) depende de {d} ({items[d]['status']}): base antes de feature")
        if not it["features"]:
            print(f"aviso: {b} não cita feature")
    # features nobody's backlog item cites back
    cited = {fid for it in items.values() for fid in it["features"]}
    for k, f in feats.items():
        for b in f["backlog"]:
            if b in items and k not in items[b]["features"] and not any(
                    k.startswith(x.split("-")[0]) for x in items[b]["features"]):
                print(f"aviso: {k} cita {b}, que não lista {k} (nem a área)")
    by_status = Counter(it["status"] for it in items.values())
    print(f"{len(feats)} features, {len(items)} itens de backlog "
          f"({', '.join(f'{s}: {n}' for s, n in by_status.items())})")
    block = summary(feats)
    print(block.replace("<!-- resumo:inicio -->\n", "").replace("\n<!-- resumo:fim -->", ""))
    if write:
        text = FEAT.read_text(encoding="utf-8")
        new = re.sub(r"<!-- resumo:inicio -->.*?<!-- resumo:fim -->", block, text, flags=re.S)
        FEAT.write_text(new, encoding="utf-8")
        print("FEATURES.md atualizado")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Convert OpenNGC's NGC.csv to the per-catalog CSVs embedded in
`crates/rp-catalog/src/data/`.

OpenNGC is the upstream source of truth (see ../src/data/LICENSE-DATA).
The script slices its `Name`/`Type`/`RA`/`Dec`/`V-Mag`/`B-Mag`/`MajAx`/
`M`/`Common names` columns into:

- `messier.csv` — one row per Messier object (~110)
- `ngc.csv` — one row per NGC entry
- `ic.csv` — one row per IC entry
- `aliases.csv` — `(alias, canonical_name)` pairs from the OpenNGC
  "Common names" column. Messier ↔ NGC mappings are *not* duplicated
  here — `messier.csv` carries its own row, so a Messier number
  resolves directly without indirection.

Re-run after upgrading OpenNGC; commit the diff so the data
provenance lives in the git history.

Usage::

    curl -sSL -o /tmp/openngc.csv \\
        https://raw.githubusercontent.com/mattiaverga/OpenNGC/master/database_files/NGC.csv
    python3 crates/rp-catalog/scripts/openngc_to_catalog.py /tmp/openngc.csv
"""

from __future__ import annotations

import csv
import re
import sys
from pathlib import Path

ra_re = re.compile(r"^(\d+):(\d+):([\d.]+)\s*$")
dec_re = re.compile(r"^([+-])(\d+):(\d+):([\d.]+)\s*$")
ngc_name_re = re.compile(r"^NGC(\d+)(.*)$")
ic_name_re = re.compile(r"^IC(\d+)(.*)$")


def parse_ra(s: str) -> float | None:
    m = ra_re.match(s.strip())
    if not m:
        return None
    h, mn, sc = m.groups()
    return float(h) + float(mn) / 60 + float(sc) / 3600


def parse_dec(s: str) -> float | None:
    m = dec_re.match(s.strip())
    if not m:
        return None
    sign, d, mn, sc = m.groups()
    v = float(d) + float(mn) / 60 + float(sc) / 3600
    return v if sign == "+" else -v


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <NGC.csv>", file=sys.stderr)
        sys.exit(2)
    src = Path(sys.argv[1])
    out_dir = Path(__file__).resolve().parent.parent / "src" / "data"
    out_dir.mkdir(parents=True, exist_ok=True)

    ngc, ic, mes = [], [], []
    aliases: list[tuple[str, str]] = []

    with src.open() as f:
        reader = csv.DictReader(f, delimiter=";")
        for row in reader:
            name = row["Name"].strip()
            type_ = row["Type"].strip()
            if type_ == "**":  # OpenNGC duplicate marker; no position
                continue
            ra = parse_ra(row["RA"])
            dec = parse_dec(row["Dec"])
            if ra is None or dec is None:
                continue
            v = row.get("V-Mag", "").strip()
            b = row.get("B-Mag", "").strip()
            mag = v or b
            majax = row["MajAx"].strip()

            canonical: str | None = None
            if name.startswith("NGC"):
                m_ = ngc_name_re.match(name)
                if not m_:
                    continue
                num = int(m_.group(1))
                suffix = m_.group(2).strip()
                canonical = f"NGC {num}" if not suffix else f"NGC {num} {suffix}"
                ngc.append([canonical, type_, f"{ra:.6f}", f"{dec:.6f}", mag, majax])
            elif name.startswith("IC"):
                m_ = ic_name_re.match(name)
                if not m_:
                    continue
                num = int(m_.group(1))
                suffix = m_.group(2).strip()
                canonical = f"IC {num}" if not suffix else f"IC {num} {suffix}"
                ic.append([canonical, type_, f"{ra:.6f}", f"{dec:.6f}", mag, majax])

            if canonical is None:
                continue

            m_num = row.get("M", "").strip()
            if m_num:
                try:
                    mname = f"M {int(m_num)}"
                except ValueError:
                    pass
                else:
                    mes.append([mname, type_, f"{ra:.6f}", f"{dec:.6f}", mag, majax])

            common = row.get("Common names", "").strip()
            if common:
                for cn in common.split(","):
                    cn = cn.strip()
                    if cn:
                        aliases.append((cn, canonical))

    header = ["name", "type", "ra_hours", "dec_degrees", "magnitude", "size_arcmin"]

    def write_csv(path: Path, rows: list[list[str]]) -> None:
        with path.open("w", newline="") as f:
            w = csv.writer(f, quoting=csv.QUOTE_MINIMAL)
            w.writerow(header)
            for r in rows:
                w.writerow(r)

    write_csv(out_dir / "ngc.csv", ngc)
    write_csv(out_dir / "ic.csv", ic)
    mes.sort(key=lambda r: int(r[0].split()[1]))
    write_csv(out_dir / "messier.csv", mes)

    aliases.sort()
    with (out_dir / "aliases.csv").open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["alias", "canonical_name"])
        w.writerows(aliases)

    print(
        f"NGC: {len(ngc)}, IC: {len(ic)}, Messier: {len(mes)}, "
        f"Aliases: {len(aliases)}"
    )


if __name__ == "__main__":
    main()

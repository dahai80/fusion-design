# Design datasets (issue #44)

Curated design-intelligence data powering `fd-search`, the fd-server
design-intel API and the `fusion-design search` CLI. All content is
authored in-repo (no upstream CSVs vendored); font names/licenses are
factual references from the Google Fonts catalog (OFL 1.1 / Apache 2.0).

## Files

| File | Rows | Content |
|------|------|---------|
| `styles.csv` | 24 | UI styles: keywords, palette hint, effects, best/avoid-for, dark-ok, complexity |
| `palettes.csv` | 24 | Palettes: mood + 8 semantic hex slots (primary/accent/bg/fg/success/warning/error/dark_bg) |
| `font_pairings.csv` | 20 | Google-Fonts pairings: mood, best-for, license |
| `ux_guidelines.csv` | 40 | do/don't pairs with severity + platform |
| `charts.csv` | 12 | Data shape → chart type with use/avoid guidance |
| `stacks.csv` | 16 | Per-stack (react/swiftui/flutter/compose/uwp/winui/wpf/avalonia/javafx/tailwind/vue/threejs) do/don't notes |

Schema pattern was inspired by nextlevelbuilder/ui-ux-pro-max-skill
(MIT) — slimmed to the columns fusion-design actually consumes.

## Provenance & CI data-QA

`provenance.json` records schema version, sources, and per-file
`{rows, sha256}`. The `fd-search` test suite (runs in `cargo test
--workspace`, enforced by CI) verifies every manifest entry against the
embedded file bytes — any dataset edit without a provenance refresh
fails the build before consumers can trust stale data.

Regenerate after editing a CSV:

```bash
python3 - <<'EOF'
import json, hashlib, csv
m = json.load(open("datasets/provenance.json"))
for e in m["files"]:
    data = open(f"datasets/{e['name']}", "rb").read()
    e["sha256"] = hashlib.sha256(data).hexdigest()
    with open(f"datasets/{e['name']}", newline="") as fh:
        e["rows"] = sum(1 for _ in csv.reader(fh)) - 1
open("datasets/provenance.json", "w").write(json.dumps(m, indent=2) + "\n")
EOF
```

## Consumers

- `GET /api/design-intel/search?q=&kind=&limit=` — server API (docs/API.md).
- `POST /api/generate` — appends a `[Design intelligence]` context block
  (top style/palette/typography/ux guardrails/chart) to the user prompt
  when the query matches the datasets. Deterministic retrieval; no
  model/network calls.
- `fusion-design search --kind <k> --query <q> [--limit N] [--json]` — CLI.
- MCP tool surface: intentionally not a bespoke layer (H-A3/P2-2 removed
  the non-compliant in-tree MCP stack); the same query API is exposed via
  the fd-server endpoint and CLI for agent pipelines.

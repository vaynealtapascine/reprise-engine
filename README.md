# reprise-engine

The relational document and layout engine behind [Reprise](https://github.com/vaynealtapascine/Reprise).
It is headless, and its editing kernel can be reused by any UI.

-   [docs/architecture.md](docs/architecture.md): the architecture decisions.
-   [docs/architecture-decisions-workbook.docx](docs/architecture-decisions-workbook.docx): the
    workbook those decisions came from.

-   [PROGRESS.md](PROGRESS.md): what is done, the known gaps and what comes next.
-   [AGENTS.md](AGENTS.md) and [docs/CODEMAP.md](docs/CODEMAP.md): how to work on it.

It is written in Rust. The end-to-end spike (decision 40) runs with:

```sh
cargo run -p reprise-cli -- spike out
```

It writes the layout before and after an edit: layout JSON, then display list JSON, SVG
and PNG for each page, and one PDF.

The test fixture font is Source Serif Pro, under the SIL Open Font License; see
[fixtures/fonts/](fixtures/fonts/).

## License

Copyright (C) 2026 Vayne Altapascine. Licensed under the GNU Affero General Public License,
version 3 or (at your option) any later version. See [LICENSE](LICENSE).

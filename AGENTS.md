# PhotoSort development

Use Graphify first for architecture, code relationships, and change-impact analysis.

1. Install development dependencies: `python -m pip install -r requirements-dev.txt`.
2. From the repository root, rebuild with `graphify extract . --code-only` after code changes. If the Windows launcher is broken, use `.venv/Scripts/python.exe -m graphify` with the same arguments. If root auto-detection selects the `photosort` package instead of this repository, pass the absolute repository path and `--out` explicitly.
3. Use `graphify query`, `graphify affected`, `graphify god-nodes`, and `graphify explain` before inspecting and changing connected code. Verify precise behavior in source and tests; the graph is not proof of correctness.
4. Keep `graphify-out/` excluded from Git. Never commit local libraries, user photographs, tokens, build output, or virtual environments.
5. Run `python -m unittest discover -s tests -v` for changes to scanning or file operations and `node --check photosort/web/app.js` for frontend changes.
6. File removal must remain recoverable. Never overwrite an existing file during restore. Preserve crash recovery and test it.

Windows application: `python -m photosort.desktop`. Browser development: `python -m photosort.server --port 18765`. Release build: `packaging/build.ps1`.

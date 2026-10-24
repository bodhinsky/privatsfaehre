# Architecture diagrams

UML views of the workspace in Mermaid, which GitHub renders inline. They model the code as of commit 6cae19c; see [`ARCHITECTURE.md`](../../ARCHITECTURE.md) for the reasoning behind it.

- [Components](components.md): Workspace crates, their dependencies and what each one touches. Dashed parts are planned (tray app, NER model).
- [Class diagram: detect](class-detect.md): Entity model, the `Recognizer` plug-in point, the NER adapter and the `Engine`.
- [Class diagram: guard runtime](class-guard-runtime.md): `cli::guard`, `clip` and `vault`: the types that run the clipboard guard.
- [Class diagram: ingest and dataset](class-ingest-dataset.md): Offline tooling behind `scan`, `label` and `eval`.
- [Sequence: clipboard detection flow](sequence-guard.md): One clipboard change in `privatsphaere guard` (review mode).

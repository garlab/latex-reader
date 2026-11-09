# Repository conventions

Treat all project content as public.

- Keep source, documentation, comments, examples, tests, and configuration free of
  personal identifiers, machine-specific absolute paths, private hostnames,
  credentials, and environment dumps. The copyright attribution in LICENSE is
  intentionally public.
- Use relative paths and generic examples. Keep large inputs, generated output,
  and local measurements outside the repository.
- Keep this crate free of rendering, file access, and application policy. It
  reads LaTeX source and reports events, contexts, numbers, and diagnostics;
  applications own layout, caches, file resolution, and revision publication.
- Preserve source ranges, bounded work, local failures, and cooperative
  cancellation. Unsupported input must stay visible and be diagnosed, never
  silently dropped.
- Describe implemented behavior and limits accurately in README.md. Planned
  features must not appear as supported.
- Run formatting, Clippy, and tests after code changes, and check the declared
  minimum Rust version.

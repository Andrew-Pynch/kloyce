# Build Report: public-kloyce

Generated: 2026-07-24
Source: internal monorepo staging snapshot (redacted path, private host)
Output: /tmp/public-kloyce

## Method

1. `rsync -a` from the internal staging snapshot into `/tmp/public-kloyce/`,
   excluding:
   - Private ops/agent docs: `PROVENANCE.md`, `AGENTS.md`, `CLAUDE.md`, `TODO.md`,
     `CONTEXT-MAP.md`, `CROSS_PLATFORM_TESTING_PROMPT.md`
   - Agent/scratch state: `.scratch/`, `planning/`, `docs/agents/`, `.agents/`,
     `.claude/`, `.codex`, `.lupeignore`
   - Secrets/build artifacts: `.env`, `.env.*`, `keys/`, `models/`, `target/`,
     `**/*.wav`, `**/*.bin`, `node_modules/`, `.git`
   - Original `.gitignore` (replaced) and `skills-lock.json` (private agent-skills
     lock file, dropped rather than shipped)
2. Wrote a strict root `.gitignore` covering build artifacts, models, media,
   secrets, keys, and scratch state.
3. Source sanitization (see below).
4. Grepped the entire output tree for the forbidden-string pattern from the
   migration contract and confirmed zero matches after fixes, including in this
   report (no literal redacted strings are reproduced above).

## Kept

`kloyce/`, `kloyce-ctl/`, `kloyce-app/`, `install/`, `packaging/`, `.github/`,
`Cargo.toml`, `dist-workspace.toml`, `deploy-macos.sh`, `LICENSE`, `README.md`,
`docs/context/` (product/domain docs, not agent-ops docs).

## Sanitization changes

- **`kloyce/src/config.rs`** — `AdvancedTranscription::default()` no longer builds
  a path under the maintainer's home directory for an external transcription
  tool. It now defaults to `transcriber_venv: PathBuf::new()` with
  `enabled: false`; the unused `home` lookup was removed. Users must configure
  their own venv path to enable the feature (documented in README as an optional
  external advanced-transcription venv, no filesystem path implied).
- **`kloyce/src/platform/linux/context.rs`** — `classify_path`'s `HOME` fallback
  changed from a hardcoded personal home path to `unwrap_or_default()` (empty
  string on unset `HOME`, no personal path baked in).
- **`kloyce/src/daemon.rs`** — test fixture audio path changed from a path under
  the maintainer's home directory to `/tmp/kloyce-test/media/recordings/pass.mp3`,
  with the matching assertion updated to the same string.
- **`kloyce/src/dictionary.rs`** — test-only context tags changed from a
  private-account-style tag to `"demo/project"` (doc comment example and both
  test usages in `test_apply_with_context`).
- Install scripts (`install/*.sh`, `deploy-macos.sh`), packaging (`packaging/**`),
  `.github/workflows/**`, and `README.md` were grepped and contained no personal
  machine/host paths or private hostnames to begin with (only the public
  `github.com/andrew-pynch/kloyce` project identity, which is the intended public
  author/repo reference, not a forbidden token).

## Verification

Final recursive case-insensitive grep over `/tmp/public-kloyce` (all files,
including this report) for the full forbidden-string pattern defined in the
migration contract — private host/machine names, tailnet identifiers, the
maintainer's home directory path, the private source-repo slug, the monorepo
name, and the personal external-tool path — returned **0 matches**.

Also confirmed absent from the tree: `.git`, `.scratch/`, `.agents/`, `.claude/`,
`.codex`, `.lupeignore`, `AGENTS.md`, `CLAUDE.md`, `TODO.md`, `CONTEXT-MAP.md`,
`CROSS_PLATFORM_TESTING_PROMPT.md`, `PROVENANCE.md`, `planning/`, `docs/agents/`,
`skills-lock.json`, `.env`, `keys/`, `models/`, `target/`, `*.wav`, `*.bin`,
`node_modules/`.

Tree: 80 files, 1.1M.

## Not done (out of scope for this phase)

- No git init / no orphan commit / no push / no GitHub repo creation.
- No changes made to the internal monorepo source snapshot.
- No formatters, linters, or project-wide test suites run.

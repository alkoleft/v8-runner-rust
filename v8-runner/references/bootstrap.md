# Bootstrapping `v8project.yaml`

Use this reference when starting `v8-runner` work in a 1С repository that has no `v8project.yaml` yet. The goal is to ask the user **only the questions you cannot answer yourself by inspecting the repository**.

`v8-runner config init` already auto-detects:
- existing source sets (Designer and EDT) under the project root,
- chosen format when `--format=auto` (default), based on what was found,
- platform version when discoverable from sources.

So in most projects the right command is simply:

```bash
v8-runner config init
```

Inspect the generated `v8project.yaml` afterwards and only re-run with explicit flags if the auto-detection was wrong or the user has constraints not visible from the filesystem.

## Decision tree (run this BEFORE asking the user)

### 1. Check whether the project is already configured

If `v8project.yaml` exists — do **not** run `config init` without `--force`. Inspect it instead and report what is configured. Reaching for `--force` requires the user’s explicit consent because it overwrites whatever local tweaks they had.

### 2. Determine source format from the filesystem

Look at the project root and immediate subdirectories.

| Filesystem signal | Likely format |
|---|---|
| `src/cf/Configuration.xml`, `src/cfe/<ext>/Configuration.xml`, raw Designer XML tree | `DESIGNER` |
| `src/cf/.project`, `src/cf/Configuration/Configuration.mdo`, `*.mdo`, `DT-INF/` | `EDT` |
| Both kinds of trees side by side (mixed mono-repo) | `auto` (default) — let `config init` register both source sets |
| Neither of the above | the repository is not a 1С source tree — stop and ask the user what they expect to find |

If the format is unambiguously one or the other, you may pass `--format=designer` or `--format=edt` for clarity, but do not have to — `auto` will pick the same.

### 3. Decide the builder backend

Defaults to `DESIGNER`. Switch to `IBCMD` only when:

- the project is `EDT` and the team is on a platform version where `ibcmd` is supported and faster (≥ 8.3.20); **and**
- there is no Designer-only feature on the critical path (some legacy project tasks still need the Designer GUI).

Ask the user only if the choice is ambiguous and you cannot tell from `tools.platform.version`. Default to `DESIGNER` when in doubt — it is the safer baseline.

### 4. Decide infobase connection

`--connection` is the connection string written into `tools.connection`. Three common shapes:

| Shape | Example | When to use |
|---|---|---|
| File infobase auto-managed by `v8-runner` | `--connection "File=build/ib"` | most local dev cycles. Path is created on first `v8-runner init`. Safe default. |
| File infobase that already exists on disk | `--connection "File=/abs/path/to/ib"` | the user has an existing baseline they want to point at |
| Server infobase | `--connection "Srvr=cluster:1541;Ref=ibname"` | central dev base, shared infobase, CI runner attached to a cluster |

For server connections there is no auto-creation (it requires DBA-level operations). The user has to confirm the database exists and credentials are stored elsewhere (`v8project.local.yaml` keeps secrets out of git).

If the project README, `docker-compose.yml`, or `.env` already describes a connection, **use that** without re-asking. If nothing is documented, ask the user **once** with the three options above and a default of `File=build/ib`.

### 5. Decide output path

Default `./v8project.yaml`. Override via `--output` only when the project is a sub-tree inside a larger repo and the user explicitly wants a non-root config. Do not invent `--output` values.

## When to ask the user (and how)

Ask only when at least one of these is true:

1. **The repository looks ambiguous or empty** — no Designer/EDT signals and no other clue. Ask: «I don’t see a Designer or EDT source tree under `<root>`. Where do the 1С sources live, or should I create a fresh empty config?»
2. **A server infobase is hinted at, but no connection string is documented.** Ask: «This looks like a server-bound project. What is the cluster:port and infobase name? Or should I default to a local `File=build/ib`?»
3. **Mixed Designer + EDT sources are detected** and the user clearly works with only one of them. Ask: «I detected both Designer and EDT trees. Should I register both as separate source sets, or only one? Which one is the primary?»
4. **`tools.platform.version` cannot be inferred** and the user has multiple installed. Ask once.

Phrase questions in a single round, give a default, and proceed if the user wants to keep defaults.

## Sample interactions

### Generic File-base project, Designer sources

```bash
v8-runner config init --connection "File=build/ib"
v8-runner init       # creates the file infobase
v8-runner build      # applies sources
```

No questions to the user.

### EDT project on platform 8.3.20+

```bash
v8-runner config init --format edt --builder IBCMD
v8-runner init
v8-runner build
```

No questions if the platform version is detected.

### Mixed Designer + EDT mono-repo

```bash
v8-runner config init       # registers both source sets via auto
```

Ask only if the user wants only one of them to be active.

### Server-bound project

```bash
# After confirming with the user:
v8-runner config init --connection "Srvr=10.0.0.10:1541;Ref=dssl_drive_ai"
# v8-runner init is NOT run — the database is managed externally
v8-runner build       # applies local sources to the existing infobase
```

Ask once for `Srvr=...;Ref=...` and credentials policy (where are they stored). Server-bound config typically lives outside git via `v8project.local.yaml`.

## After `config init`

Always inspect the generated `v8project.yaml` before running mutating commands. Show the user:

- the picked format and builder (`format`, `builder`),
- detected source sets (`source-sets[*].name`, `path`),
- the connection string,
- whether `tools.platform.version` was filled,
- any warnings printed by `config init` (often hint at missing tools or unusual layouts).

If something looks off, reach for explicit `--format`, `--builder`, or `--connection` rather than editing the YAML by hand — that keeps the workflow reproducible.

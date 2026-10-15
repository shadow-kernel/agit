# agit: Überblick (Deutsch)

Dieser Text ist die deutsche Zusammenfassung des englischen Handbuchs in [`docs/handbook/`](../README.md). Er beschreibt agit 0.1.0 im Stand vom 15. Oktober 2026 (Arbeitskopie `b1fc6ff`). Alle gezeigten Befehle und Ausgaben stammen aus einer echten Sitzung mit den gebauten Binärdateien; lange Ids sind gekürzt. Für Details verweist jeder Abschnitt auf das jeweilige englische Kapitel.

## Inhalt

1. [Was ist agit](#1-was-ist-agit)
2. [Besonderheiten gegenüber git](#2-besonderheiten-gegenüber-git)
3. [Wie es funktioniert](#3-wie-es-funktioniert)
4. [Verwendung für Entwickler](#4-verwendung-für-entwickler)
5. [Verwendung für Agenten](#5-verwendung-für-agenten)
6. [Server und Plattform](#6-server-und-plattform)
7. [Wissensdatenbank](#7-wissensdatenbank)
8. [Claude-Integration](#8-claude-integration)
9. [SDKs und Web-Oberfläche](#9-sdks-und-web-oberfläche)
10. [Stand, Grenzen und Roadmap](#10-stand-grenzen-und-roadmap)
11. [Lesereihenfolge](#11-lesereihenfolge)

## 1. Was ist agit

agit ist ein Versionskontrollsystem und eine Hosting-Plattform, die für eine Welt gebaut sind, in der die meisten Commits von KI-Agenten stammen. Es behält das Denkmodell und das Befehlsvokabular von git (`init`, `add`, `commit`, `branch`, `merge`, `push`, `pull`, `clone`) und ersetzt die Maschinerie darunter durch eine, die für Programme entworfen ist, die parallel arbeiten, abstürzen, blind wiederholen und sich ohne Menschen abstimmen müssen.

Drei Dinge machen agit aus:

- **Ein lokales VCS** in Rust: BLAKE3-Hashes, zstd-komprimierte Objekte in anhängbaren Pack-Segmenten, binäre Trees mit Dateigrößen, Commits als JSON. `status`, `diff`, `log`, `blame`, `stash`, `revert`, `cherry-pick`, `reflog`, `reset`, `gc`, `grep` und Submodule verhalten sich so, wie ein git-Nutzer es erwartet.
- **Ein Maschinenvertrag**: Jeder Befehl akzeptiert `--json` und liefert genau einen Umschlag, `{"ok": true, "result": …}` oder `{"ok": false, "error": {"code", "message", "retryable", "details"}}`. Exit-Codes sind 0 (Erfolg), 1 (Fehler), 3 (Konflikt: Merge, Lease, schmutziger Arbeitsbaum) und 75 (wiederholbar: Ref-Konflikt, Lock-Timeout). Ein Agent verzweigt auf `error.code`, nie auf Prosa.
- **Eine Plattform**: `agit-server` hostet Repositories über HTTP (`/api/v1`) mit Compare-and-Swap-Refs, Pull Requests mit typisierten Kommentaren und Reviews, Richtlinien (Policies), serverweiten Leases, einem Ereignisstrom (SSE), signierten Webhooks, einem Root-Token plus bereichsbeschränkten Tokens mit Audit-Log und einer Wissensdatenbank. Darauf sitzen eine React-Web-Oberfläche, ein Python- und ein TypeScript-SDK, ein MCP-Server (`agit mcp`) und eine Integration für Claude Code.

Zielgruppen: Entwickler, die git kennen und agit täglich nutzen wollen; Autoren und Betreiber von Agenten; Operatoren, die einen Server für ein Team betreiben; Mitwirkende, die die Formate verstehen wollen. Siehe [0. Introduction](../00-introduction.md).

## 2. Besonderheiten gegenüber git

Das englische Kapitel [1. Why](../01-why.md) führt zehn konkrete Schwächen von git im Agentenbetrieb auf und die Antwort von agit auf jede. Kurz:

| Problem mit git | Antwort von agit |
| --- | --- |
| Prosa-Ausgabe, die mit Regex geparst wird | `--json` überall, stabile Fehlercodes, `retryable`-Flag, Exit-Code-Klassen |
| Verwaiste `.lock`-Dateien nach Abstürzen | advisory `flock`; der Kernel gibt die Sperre frei, nichts bleibt zurück |
| Konflikte erst beim Merge, nachdem beide Agenten ihr Budget verbraucht haben | **Leases**: zeitlich begrenzte, glob-basierte Ansprüche auf Pfade; `agit commit` verweigert Änderungen unter dem exklusiven Lease eines anderen |
| Letzter Schreiber gewinnt bei Refs | **Compare-and-Swap** bei jeder Ref-Änderung, atomare Mehrfach-Ref-Transaktionen, `ref_conflict` ist wiederholbar |
| Commit-Nachricht als einzige Metadaten | strukturierte Felder `intent`, `task`, `agent{id, model, run_id, on_behalf_of}`, `meta`, optionale Ed25519-Signatur |
| Keine billigen Schnappschüsse | **Checkpoints**: ganzer Arbeitsbaum inklusive unversionierter Dateien auf `refs/checkpoints/<workspace>`, außerhalb der Branch-Historie |
| Agenten pollen `status` und `log` | **Ereignisprotokoll** `events.jsonl` mit Sequenznummern, `agit events --follow`, SSE und Webhooks auf dem Server |
| Schwerfällige `git worktree` | **Workspaces**: viele Arbeitsverzeichnisse, ein Objektspeicher, je Agent ein Branch `ws/<name>` |
| Wissen liegt in Wikis und Chats | **Notizen** mit typisierten Tags direkt am Code, Tag-Regeln, Veralterungserkennung, Entwürfe aus Merges, Kontextpaket pro Pfad |
| SHA-1, langsamer `status` | BLAKE3, Größen im Tree, Stat-Cache und Tree-Cache im Index; gemessen: `add`+`commit` 12,5 ms statt 34,6 ms |

Ein Beispiel für die Lease-Verweigerung, so wie sie tatsächlich aussieht:

```sh
$ agit lease claim src --ttl 30m --note "Config laden (DEMO-7)" --holder coder-1
lease 3bd2ec0d8a15 granted to coder-1 for src
$ agit lease claim 'src/**' --holder coder-2 --json; echo "exit=$?"
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: src/** is held by coder-1 (lease 3bd2ec0d8a15)","retryable":false,"details":{"path":"src/**","holder":"coder-1","lease_id":"3bd2ec0d8a15"}}}
exit=3
```

## 3. Wie es funktioniert

Die Grundbegriffe aus [5. Concepts](../05-concepts.md) und die Interna aus [14. Architecture](../14-architecture-internals.md):

**Der JSON-Vertrag.** Jeder Befehl, der mit `--json` (oder `AGIT_JSON=true`) läuft, schreibt genau einen Umschlag auf die Standardausgabe. Erfolg:

```json
{"ok": true, "result": {"id": "edf2c5ac…", "tree": "560556cb…", "parents": ["44b9a0f2…"], "branch": "feature/config", "changes": [{"path": "src/config.rs", "kind": "added", "new": {"id": "…", "mode": "file", "size": 126}}], "summary": "Load the greeting from Config"}}
```

Fehler, mit stabilem `code`, dem Flag `retryable` und strukturierten `details`, wo es sie gibt:

```json
{"ok": false, "error": {"code": "ref_conflict", "message": "reference refs/heads/scratch changed concurrently (expected None, found Some(ObjectId(3bf5d8…)))", "retryable": true, "details": {"name": "refs/heads/scratch", "expected": null, "actual": "3bf5d89c…"}}}
```

Die Codes des Kerns (`not_a_repository`, `already_exists`, `object_not_found`, `corrupt_object`, `hash_mismatch`, `ref_not_found`, `invalid_ref_name`, `ref_conflict`, `invalid_object_id`, `ambiguous_revision`, `unknown_revision`, `path_outside_worktree`, `path_not_tracked`, `nothing_to_commit`, `dirty_worktree`, `merge_conflicts`, `lease_conflict`, `lease_not_found`, `workspace_exists`, `workspace_not_found`, `lock_timeout`, `policy_violation`, `not_found`, `forbidden`, `invalid`, `io`, `serde`) sind in CLI, HTTP-API, MCP und SDKs identisch. Nur `ref_conflict` und `lock_timeout` sind wiederholbar. Fehler der reinen CLI-Schicht (unbekannter Konfigurationsschlüssel, fehlende Argumente) tragen den Code `error`.

**Verzeichnis `.agit/`.**

```
.agit/
  config.json              format_version, default_branch, user, remotes
  HEAD, index              primärer Workspace (Index im Format AGI2)
  objects/pack/*.agp|.idx  Pack-Segmente (AGP1) mit veröffentlichten Indizes (AGX1)
  objects/xx/…             lose Objekte älterer Versionen (AGO1), weiterhin lesbar
  refs/heads|tags|checkpoints|remotes/
  logs/HEAD, logs/refs/…   Reflogs
  leases/<id>.json         aktive Leases
  events.jsonl             Ereignisprotokoll mit seq
  locks/                   flock-Dateien
  workspaces/<name>/       HEAD, index, meta.json sekundärer Workspaces
  knowledge/               lokale Notizen und Cache (n_local_*, rules.json)
  git-map.json             Abbildung git-SHA → agit-Id nach einem Import
```

**Objekte und Ids.** `id = BLAKE3-256(kind_tag || payload)` mit den Kennungen 1 (Blob), 2 (Tree), 3 (Commit); 64 Hex-Zeichen. Trees sind binär (`AGT1`) und speichern pro Eintrag Modus, Id, Größe und Namen; Commits sind kanonisches JSON. Jeder Lesezugriff hasht nach und vergleicht; beschädigte Objekte werden als `hash_mismatch` gemeldet, nie ausgeliefert.

**Speicher.** Objekte werden an ein prozesseigenes Pack-Segment angehängt (`objects/pack/seg-<pid>-<zufall>.agp`) und erst mit der atomar veröffentlichten Indexdatei (`.idx`, `AGX1`) für andere Prozesse sichtbar. Die Veröffentlichung passiert vor jeder Ref-Änderung und jedem Index-Speichern (Regel "publish on flush"), sodass eine Referenz nie auf ein unsichtbares Objekt zeigt. Alte lose Objekte (`AGO1`) werden weiterhin gelesen; `agit gc` packt sie um. Der Index hat das Format `AGI2` mit Stat-Cache und zwischengespeicherten Verzeichnis-Tree-Ids.

**Dauerhaftigkeit.** `AGIT_FSYNC=refs` (Standard) synchronisiert Ref-, Index- und Log-Schreibungen, Objekte werden atomar, aber ohne eigenes fsync geschrieben (wie bei git); `all` synchronisiert alles; `none` nichts. Die eine fsync-Operation der Ref-Datei dominiert die Kosten eines Commits; ein Journal mit Gruppen-Commit, das sie amortisiert, ist geplant.

**Sperren.** Drei `flock`-Dateien unter `.agit/locks/` (Refs 10 s, Leases und Events 5 s Timeout). Ref-Änderungen sind Compare-and-Swap unter dieser Sperre; `update_many` prüft erst alle Erwartungen, schreibt dann alle Refs.

**Workspaces.** Ein Workspace ist ein Paar aus `HEAD` und Index, das alles andere in `.agit/` teilt. `agit ws new coder-2` legt ein Geschwisterverzeichnis `<repo>.coder-2` auf dem Branch `ws/coder-2` an; dort liegt eine `.agit`-*Datei*, die auf das gemeinsame Verzeichnis zeigt.

**Leases.** JSON-Dateien unter `.agit/leases/`: Halter, Pfadmuster, Modus (`exclusive`/`shared`), Ablaufzeit. Ein literaler Pfad deckt sich selbst und alles darunter ab; Globs decken einen Pfad ab, wenn sie ihn oder einen seiner Vorfahren treffen. Ablauf ist träge (keine Hintergrundprozesse, kein Ablaufereignis).

**Checkpoints.** Ein Commit mit `checkpoint: true`, dessen Elternteil HEAD ist und der über `meta.agit.previous_checkpoint` auf den vorigen Checkpoint verweist. `agit log` zeigt sie nie; `agit restore <id>` stellt den ganzen Baum wieder her.

**Merge.** Drei-Wege-Merge über flachgeklopfte Bäume, textuell im diff3-Stil mit Markern inklusive Basisversion; Konfliktarten `content`, `delete_modify`, `add_add`, `mode`, `binary`. Die Merge-Basis wird mit einer begrenzten Breitensuche bestimmt. Server-Merges, `revert`, `cherry-pick` und `stash apply` nutzen denselben Algorithmus.

**Blame** folgt der First-Parent-Historie der Commits, die den Pfad berührt haben (maximal 500); Zeilen von Seitenzweigen werden daher dem Merge-Commit zugeschrieben.

**git-Brücke.** `agit git import` liest mit `git rev-list`/`cat-file`/`for-each-ref`, hebt die Trailer `Intent:`, `Task:`, `Agent:`, `Model:` in strukturierte Felder, bewahrt die SHA in `meta["git.sha"]` und führt eine Abbildungstabelle `.agit/git-map.json`; `agit git export` schreibt über `git fast-import` zurück, inkrementell.

## 4. Verwendung für Entwickler

Das vollständige Protokoll einer ersten Sitzung steht in [3. Quick start for developers](../03-quickstart-developers.md). Die wichtigsten Schritte:

```sh
$ agit init
$ agit config user.name "Dana Developer" && agit config user.email dana@example.com
$ agit add README.md src
$ agit commit -m "Initial commit" --intent "Bootstrap the demo project" --task DEMO-1
[main f61ab45c8fb6] Initial commit
 2 file(s) changed
$ agit status --json
{"ok":true,"result":{"workspace":"main","branch":"main","head":"f61ab45c…","staged":[],"unstaged":[],"untracked":[],"conflicts":[],"merge_in_progress":false}}
```

Branches und ein Merge-Konflikt:

```sh
$ agit branch feature/greeting && agit switch feature/greeting
$ …                                    # auf beiden Branches dieselbe Zeile ändern
$ agit switch main && agit merge feature/greeting --json; echo "exit=$?"
{"ok":true,"result":{"kind":"conflicts","tree":"84c98e13…","conflicts":[{"path":"src/main.rs","kind":"content","base":"…","ours":"…","theirs":"…"}],"auto_merged":[]}}
{"ok":false,"error":{"code":"merge_conflicts","message":"merge produced conflicts in 1 file(s)","retryable":false,"details":{"count":1}}}
exit=3
$ cat src/main.rs                      # diff3-Marker mit Basisversion
$ …                                    # auflösen
$ agit add src/main.rs && agit commit -m "Merge feature/greeting" --intent "Combine both greetings"
$ agit log --graph --oneline
* a109bac6bf39 Merge feature/greeting
|\
* | e1f412639dfe Greet agit
| * 351a8c03e737 Greet the world
| |/
* 707464cd6b1e Second
* f61ab45c8fb6 Initial commit
```

Checkpoints und Reflog im Alltag:

```sh
$ agit checkpoint -m "before rewriting helper" --json
{"ok":true,"result":{"id":"4cc2e92e…","tree":"b37c8a4e…","previous":null,"changes_vs_head":2,"created":true}}
$ …                                    # der riskante Schritt geht schief
$ agit checkpoint restore 4cc2e92e…    # ganzer Baum zurück, auch unversionierte Dateien
$ agit reflog -n 2
a109bac6bf39 HEAD@{0}: reset: moving to HEAD~1 (danielbrueckmann, 2026-10-15T19:30:01Z)
fa32a4268082 HEAD@{1}: commit: Add helper and README note (danielbrueckmann, 2026-10-15T19:30:01Z)
$ agit reset --hard fa32a426808261a81cdefc55d7367c5e40a3177aed30b8a6b0973fec1e6914f6
HEAD is now at fa32a4268082 (Hard)
```

Ein zweiter Workspace für einen Agenten und ein Lease, das einen fremden Commit verhindert:

```sh
$ agit ws new coder-2 --agent coder-2 --json
{"ok":true,"result":{"name":"coder-2","path":"/…/demo.coder-2","created_ms":1792092645238,"agent":"coder-2"}}
$ agit lease claim src/main.rs --holder coder-1 --ttl 30m --note "tidy main"
lease c618a7ee7a6c granted to coder-1 for src/main.rs
$ agit commit -a -m "Punctuation" --intent "Add exclamation mark" --agent coder-2 --json; echo "exit=$?"
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: src/main.rs is held by coder-1 (lease c618a7ee7a6c)","retryable":false,"details":{"path":"src/main.rs","holder":"coder-1","lease_id":"c618a7ee7a6c"}}}
exit=3
$ agit events --kind lease. --json | tail -n 1
{"seq":30,"time_ms":1792092645333,"kind":"lease.release","workspace":"main","actor":"coder-2","data":{"id":"8397196cea77","paths":["docs/**"]}}
```

Weitere Alltagsbefehle, alle im Kapitel mit echter Ausgabe belegt: `agit stash` (inklusive unversionierter Dateien), `agit blame`, `agit reflog` und `agit reset --hard <id>` zum Rückgängigmachen (die Anzeige `HEAD@{n}` ist nur Darstellung, zurück geht es über die Id), `agit checkpoint` / `agit restore`, `agit revert`, `agit cherry-pick`, `agit grep`, `agit tag`, `agit gc --dry-run`, `agit fsck`, `agit submodule add|status|update`, `agit git import <git-repo> --into <ziel>` und `agit git export`.

Die Referenz aller Befehle mit Flags, JSON-Formen, Exit-Codes und Umgebungsvariablen (`AGIT_AGENT`, `AGIT_MODEL`, `AGIT_RUN_ID`, `AGIT_JSON=true`, `AGIT_TOKEN`, `AGIT_SIGN`, `AGIT_KEY`, `AGIT_KEYS_DIR`, `AGIT_FSYNC`, `AGIT_PACK`, `AGIT_AUTHOR_NAME/EMAIL`) ist [6. CLI reference](../06-cli-reference.md). Installation (cargo ab Rust 1.89, `scripts/install.sh`, Docker-Image `ghcr.io/shadow-kernel/agit`, Homebrew-Vorlage; keine Shell-Vervollständigung in 0.1.0) in [2. Installation](../02-installation.md).

## 5. Verwendung für Agenten

Die Schleife, um die agit gebaut ist, lautet: **Kontext lesen, beanspruchen, was man ändern wird, vor Risiken einen Checkpoint setzen, mit Absicht committen, einen Pull Request öffnen, Gelerntes festhalten, freigeben.** [4. Quick start for agents](../04-quickstart-agents.md) führt sie gegen einen laufenden Server vor; [docs/AGENT-GUIDE.md](../../AGENT-GUIDE.md) ist die Einseiten-Fassung für einen System-Prompt.

```sh
$ export AGIT_AGENT=coder-1 AGIT_MODEL=demo-model-1 AGIT_RUN_ID=run-2026-10-15-a AGIT_TOKEN=…
$ agit know context src/main.rs                 # Notizen, vorgeschlagene Tags, Commits, Leases, offene PRs, aktive Agenten
$ agit lease claim src --ttl 30m --note "Config laden (DEMO-7)"
$ agit switch -c feature/config
$ agit checkpoint -m "vor dem Hinzufügen von config"
$ …                                             # bearbeiten
$ agit add -A
$ agit commit -m "Load the greeting from Config" --intent "Make the greeting configurable for DEMO-7" --task DEMO-7
$ agit pr create --push -t "Load greeting from Config" --intent "…" --task DEMO-7 --json
{"ok":true,"result":{"number":1,"title":"Load greeting from Config",…,"source":"refs/heads/feature/config","target":"refs/heads/main","author":{"id":"coder-1"},"state":"open",…}}
```

Ein zweiter Agent prüft (`reviewer-1`): typisierte Kommentare (`question`, `suggestion`, `blocker`, `note`) mit Pfad und Zeile, dann `agit pr review 1 --decision request_changes|approve|comment`. Ein Operator schützt `main` per Policy (nur über PRs, eine Freigabe, Absicht und Agentenidentität Pflicht). Direkte Pushes und ein verfrühter Merge werden dann mit `policy_violation` abgelehnt; nach dem Auflösen des Blockers, einem Check (`tests: success`) und der Freigabe merged der Server mit Compare-and-Swap auf `main`.

Der Ablauf des Pull Requests im Einzelnen, wie er in der Sitzung lief:

```sh
$ AGIT_AGENT=reviewer-1 agit pr comment 1 --kind blocker --path src/config.rs --line 4 -b "Hard-coded greeting; read it from the environment." --json
{"ok":true,"result":{"id":1,"author":"reviewer-1","path":"src/config.rs","line":4,"kind":"blocker","body":"…","resolved":false,"time_ms":…}}
$ AGIT_AGENT=reviewer-1 agit pr review 1 --decision request_changes -b "See blocker on config.rs"
$ AGIT_AGENT=reviewer-1 agit pr merge 1 --json; echo "exit=$?"
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: policy violation: a reviewer requested changes","retryable":false,"details":null}}
exit=1
$ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'X-Agit-Agent: reviewer-1' …/pulls/1/comments/1/resolve >/dev/null
$ curl -s -X POST … -H 'X-Agit-Agent: ci-bot' …/pulls/1/checks -d '{"name":"tests","status":"success","url":"https://ci.example/run/42"}' >/dev/null
$ AGIT_AGENT=reviewer-1 agit pr review 1 --decision approve -b "Resolved; fine as a first step."
$ AGIT_AGENT=reviewer-1 agit pr merge 1 --json | jq -c '.result | {state: .pr.state, merged_commit: .merged_commit[0:12]}'
{"state":"merged","merged_commit":"3bf5d89c19f4"}
```

Der Merge erzeugt zusätzlich eine Entwurfsnotiz in der Wissensdatenbank (Abschnitt 7) und ein Ereignis `pull.merge`, das Webhooks und SSE-Abonnenten erhalten.

Signaturen: `agit key generate --register` erzeugt einen Ed25519-Schlüssel, der nach dem Agenten benannt ist, und registriert den öffentlichen Teil im Repository; danach wird jeder Commit automatisch signiert (`agit verify`, Server meldet `trusted: true`). Mit `require_signed` in der Policy kommt kein unsignierter Commit mehr auf `main`.

Fehlerbehandlung nach Code, nicht nach Nachricht:

| `error.code` | Exit | Bedeutung und Reaktion |
| --- | --- | --- |
| `ref_conflict`, `lock_timeout` | 75 | wiederholbar: `agit pull`, neu prüfen, erneut versuchen |
| `lease_conflict` | 3 | anderer Agent hält den Pfad: andere Arbeit wählen, auf `lease.release` warten oder nachfragen; nicht erzwingen |
| `merge_conflicts` | 3 | markierte Dateien auflösen, `agit add`, committen |
| `dirty_worktree` | 3 | erst Checkpoint oder Commit |
| `policy_violation` | 1 | Nachricht lesen: fehlende Absicht, Freigabe, Signatur oder direkter Push auf einen geschützten Branch; PR öffnen |
| `invalid` beim Push | 1 | Non-Fast-Forward: `agit pull`, dann erneut pushen |

Muster für Agentenflotten (ein Workspace je Agent, Lease-Etikette, Checkpoint-Rhythmus, Reviewer-Agenten, Policy-Vorlagen für Einzelentwickler, kleines Team und streng, Wiederherstellung über Reflog, Reset und Restore) stehen in [12. Multi-agent workflows](../12-multi-agent-workflows.md).

## 6. Server und Plattform

`agit-server` ist eine statische Binärdatei ohne Datenbank; der gesamte Zustand liegt unter `--data-dir`:

```sh
$ export AGIT_SERVER_TOKEN=$(openssl rand -hex 32)
$ agit-server --data-dir /var/lib/agit --listen 127.0.0.1:7878
$ curl -s http://127.0.0.1:7878/api/v1/health
{"status":"ok","version":"0.1.0"}
```

Authentifizierung: ein Root-Token (`AGIT_SERVER_TOKEN`) mit allen Rechten, dazu bereichsbeschränkte Tokens über `POST /auth/tokens` mit `scopes: [{repo, access: read|write|admin}]`, optionaler Bindung an eine Agentenidentität (die dann den Header `X-Agit-Agent` überschreibt) und Ablaufzeit; jede Änderung und jede Ablehnung landet in `audit.jsonl`. Ohne Root-Token läuft der Server im offenen Modus (nur für `127.0.0.1`). TLS, Ratenbegrenzung und Größenlimits übernimmt ein Reverse-Proxy (Caddy- und nginx-Beispiele in `deploy/`); für den SSE-Pfad muss die Antwortpufferung aus sein, für `agit push` das Body-Limit hoch.

Eine Policy, die `main` schützt, und die Reaktion des Servers auf einen direkten Push:

```sh
$ curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    http://127.0.0.1:7878/api/v1/repos/demo/policy \
    -d '{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true}'
{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true,"require_checks":false,"block_on_blockers":true,"allowed_merge_agents":[],"allowed_push_agents":[],"max_files_per_commit":0,"require_signed":false}
$ curl -s -X POST … /repos/demo/refs/update -d '{"updates":[{"name":"refs/heads/main","expect":{"any":true},"new":"edf2c5ac…"}]}'
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: refs/heads/main is protected; changes must go through a pull request","retryable":false}}
```

Regeln: geschützte Branches können nicht gelöscht werden; mit `require_pull_request` bewegen sie sich nur durch PR-Merges; Freigaben zählen die letzte Entscheidung je Reviewer ohne den Autor; jedes `request_changes` blockiert; ungelöste `blocker`-Kommentare blockieren, solange `block_on_blockers` gesetzt ist; `require_checks` verlangt mindestens einen Check und alle `success`; `require_intent`, `require_agent`, `require_signed` und `max_files_per_commit` gelten für jeden neuen Commit, den die Änderung einführt.

Datenverzeichnis des Servers:

```
<data-dir>/
  audit.jsonl              abgelehnte Anfragen und alle Änderungen (principal, agent, method, path, status)
  auth/                    bereichsbeschränkte Tokens (nur Hashes)
  repos/<repo>/
    meta.json, objects/pack/, refs/, logs/, events.jsonl, leases/
    pulls/<n>.json          Pull Requests mit Kommentaren, Reviews, Checks
    knowledge/notes/, knowledge/rules.json
    policy.json, keys.json, webhooks.json (enthält Geheimnisse im Klartext)
    locks/
```

Weitere Verwaltung: Policies (`GET/PUT /policy`), Schlüsselregister (`/keys`), Webhooks mit Kind-Filtern, drei Zustellversuchen und `X-Agit-Signature: blake3=…`, Ereignisse per Polling, SSE oder Webhook, `GET /agents` für die Aktivität. Backup ist eine Kopie des Datenverzeichnisses (Dateisystem-Snapshot, Stop-Copy-Start oder zweimaliges `rsync`), verschlüsselt, weil `webhooks.json` Geheimnisse enthält. Upgrades: Changelog lesen, sichern, Binärdatei tauschen, neu starten, `health` und `agit pull` prüfen. Alles in [13. Server administration](../13-server-administration.md); die HTTP-Schnittstelle mit ausgeführten `curl`-Beispielen (Fehlerumschlag, CAS-Semantik, Objektströme, SSE, PR-Lebenszyklus, Policy, Schlüssel, Webhooks, Notizen, Kontextpakete) in [8. HTTP API](../08-http-api.md).

## 7. Wissensdatenbank

Die Wissensdatenbank hält Entscheidungen, Fallstricke, Spezifikationen und Anleitungen dort, wo der Agent sie braucht: am Pfad, den er gerade ändert. [11. Knowledge base](../11-knowledge-base.md) ist die Vertiefung.

- **Notiz**: Titel, Markdown-Text, typisierte Tags, Pfade (auch Globs), Commits, PRs, Autor, Vertrauenswert, `valid_at` (Commit, gegen den sie geschrieben oder zuletzt bestätigt wurde), `stale`, `draft`.
- **Tags** haben die Form `namespace:value`. Empfehlung: `area:` (Modul oder Domäne, per Regel vergeben), genau ein `kind:` (`decision`, `gotcha`, `spec`, `howto`, `fact`, `rule`; `change` für Entwürfe), `lang:` und `service:` per Regel. Der Server akzeptiert derzeit auch Tags ohne Namespace, worauf man sich nicht verlassen sollte.
- **Tag-Regeln** (`PUT /knowledge/rules`) bilden Pfadmuster auf Tags ab; der Kontextaufruf liefert sie als `suggested_tags`.
- **Kontextpaket**: `agit know context <pfad>` liefert in einem Aufruf die abdeckenden Notizen (frische vor veralteten), vorgeschlagene Tags, letzte Commits, überlappende Leases, offene PRs und aktive Agenten; mit `include=files&budget=` auch die Datei und bis zu fünf Nachbarn, auf das Budget gekürzt. Notiztexte sind nie enthalten, nur 200-Zeichen-Auszüge.
- **Veralterung**: `stale` wird bei jedem Lesen berechnet; eine Notiz ist veraltet, wenn ein abgedeckter Pfad sich zwischen `valid_at` und HEAD geändert hat. `GET /knowledge/stale` listet sie mit `changed_paths`; nach Prüfung bestätigt `POST /notes/{id}/validate` sie an HEAD (ein `PATCH` allein hebt die Veralterung nicht auf).
- **Entwürfe**: Jeder gemergte PR erzeugt eine versteckte Entwurfsnotiz (Titel des PR, Absicht, Aufgabe, geänderte Dateien, Reviews; `kind:change` plus Regel-Tags; Vertrauen 0,4), die mit `accept` (optional bearbeitet) übernommen oder gelöscht wird.
- **Suche**: BM25 über Titel (Gewicht 3), Tags (2), Text und Pfade (1); camelCase und snake_case werden zerlegt; Begriffe ab drei Zeichen treffen auch als Präfix. Kein Stemming, keine Synonyme, keine Embeddings.
- **Offline**: `.agit/knowledge/` speichert lokale Notizen (`n_local_*`) und einen Cache der Server-Notizen und Regeln; bei Verbindungsfehlern antwortet der lokale Speicher (`source: "local"`), `agit know sync` lädt lokale Notizen hoch und erneuert den Cache, `--local` erzwingt den lokalen Betrieb.

Beispiel aus der Sitzung:

```sh
$ agit know add -t "Config::load() is a stub" --tag kind:gotcha --tag area:core --tag lang:rust \
    --path src/config.rs --commit HEAD --confidence 0.8 -b "The greeting is hard-coded until DEMO-8 …" --json
{"ok":true,"result":{"note":{"id":"n_1f5d3dce7c3b","title":"Config::load() is a stub",…,"valid_at":"3bf5d89c…","stale":false},"source":"server"}}
$ curl -s -H "$H" $U/repos/demo/knowledge/stale | jq -c '.[] | {title, changed_paths}'      # nach PR #4
{"title":"Config::load() is a stub","changed_paths":["src/config.rs"]}
```

Vorlagen für gute Notizen (Gotcha, Entscheidung, Spezifikation, Anleitung), Abfragestrategien für Agenten und die heutigen Grenzen stehen im englischen Kapitel.

## 8. Claude-Integration

agit bietet zwei Hälften für Claude Code, beschrieben in [7. MCP reference](../07-mcp-reference.md) und [docs/CLAUDE-CODE.md](../../CLAUDE-CODE.md):

**MCP-Server.** `agit mcp` spricht das Model Context Protocol über stdio und stellt 33 Werkzeuge bereit, die dieselben JSON-Ergebnisse und Fehlercodes liefern wie die CLI: `agit_status`, `agit_log`, `agit_show`, `agit_diff`, `agit_read_file`, `agit_ls`, `agit_add`, `agit_commit`, `agit_checkpoint`, `agit_checkpoint_list`, `agit_restore`, `agit_branches`, `agit_switch`, `agit_merge`, `agit_lease_claim|release|list|check`, `agit_events`, `agit_workspaces`, `agit_push`, `agit_pull`, `agit_pr_create|list|view|review|comment|merge`, `agit_context`, `agit_know_search|get|add|sync`. Registrierung:

```sh
claude mcp add --scope project agit -e AGIT_AGENT=claude-code -- agit mcp
```

Der `instructions`-String des Servers beschreibt die empfohlene Reihenfolge: `agit_context(path)` vor dem Bearbeiten, `agit_lease_claim` für die Pfade, die man ändern wird, `agit_checkpoint` vor riskanten Änderungen, `agit_add` + `agit_commit` mit Absicht, Aufgabe und Agent, `agit_pr_create` zum Abschluss, `agit_know_add` für Gelerntes. Fehlgeschlagene Aufrufe kommen als `isError: true` mit dem Fehlerumschlag zurück.

**Hooks.** Fünf Bash-Skripte in `integrations/claude-code/`, die die Disziplin deterministisch machen, weil ein Modell sie vergessen kann: `pre-edit.sh` blockiert Bearbeitungen auf Pfaden unter dem exklusiven Lease eines anderen Agenten (Exit 2) und injiziert das Kontextpaket; `post-edit.sh` beansprucht ein 30-Minuten-Lease auf jede bearbeitete Datei; `checkpoint.sh` setzt am Ende jeder Runde einen Checkpoint, wenn der Baum schmutzig ist; `session-start.sh` liefert die Einweisung mit Zustand, Leases und Schleife; `pre-commit-guard.sh` blockiert `agit commit` ohne `--intent` und erinnert bei `git commit`/`git push`. `install.sh` kopiert die Skripte nach `.claude/hooks/agit/`, mischt `settings.json`, setzt `AGIT_AGENT`, hängt `CLAUDE.md.snippet` an `CLAUDE.md` an und schreibt mit `--mcp` die `.mcp.json`; `--global` installiert nach `~/.claude`, `--uninstall` entfernt genau das Installierte. Der Selbsttest läuft gegen die 0.1.0-Binärdatei durch (`passed: 100 failed: 0`).

## 9. SDKs und Web-Oberfläche

**Python** (`agit-sdk`, Import `agit`, ohne Abhängigkeiten, Python 3.10+) und **TypeScript** (`@agit/sdk`, ESM und CJS, Node 18+/Deno/Bun/Browser) decken jede Endpunkt von `docs/API.md` ab, werfen typisierte Fehler mit `code`, `retryable` und `details`, streamen SSE und bringen `AgentSession` mit: Kontextpaket, Leases beanspruchen (und beim Verlassen freigeben, auch bei Ausnahmen; in TypeScript `withLeases`), PR öffnen, Notizen anlegen, veraltete Notizen prüfen. Keines der SDKs berechnet BLAKE3, Commits entstehen also weiterhin mit der CLI oder über MCP.

```python
from agit import Client, AgentSession, LeaseConflict

client = Client("http://127.0.0.1:7878/api/v1", token="…", agent="coder-2", model="demo-model-2")
with AgentSession(client, "demo", agent_id="coder-2") as s:
    ctx = s.context_pack("src/config.rs", budget=800)          # Notizen, Tags, Commits, Leases, aktive Agenten
    lease = s.claim(["src/config.rs"], ttl_secs=600, note="SDK walkthrough")
    try:
        client.claim_lease("demo", holder="coder-9", paths=["src/**"], ttl_secs=60)
    except LeaseConflict as e:
        print("coder-9 blockiert:", e.path, "gehalten von", e.holder)   # lease_conflict, nicht wiederholbar
    s.remember("Config is read once at startup", body="…", tags=["kind:fact", "area:core"], paths=["src/main.rs", "src/config.rs"])
# Leases werden hier freigegeben, auch nach einer Ausnahme
``` Beide Durchläufe mit echter Ausgabe stehen in [9. SDKs](../09-sdks.md).

**Web-Oberfläche** (`web/`, Vite, React, TypeScript): Repository-Liste, Code-Browser mit Syntaxhervorhebung und Dateifinder, Blame im Browser, Commits mit Agent-, Absicht- und Aufgabenfiltern, Commit-Detail mit Diff und Notizen, Vergleichsansicht mit Knopf zum PR, Suche (Code, Commits, Notizen), Pull Requests mit Unterhaltung, Inline-Kommentaren, Checks und Merge, Agenten-Tabelle mit Live-Ereignisstrom, Lease-Tafel mit Countdown, Wissensdatenbank mit Kontextpanel, Einstellungen für Policy, Webhooks, Tag-Regeln und Gefahrenzone; Benachrichtigungsglocke, Hell- und Dunkelmodus, Mock-Modus ohne Server (`VITE_AGIT_MOCK=1`). Der Server liefert das gebaute Bundle mit `--web-dir web/dist` samt SPA-Fallback aus. Siehe [10. Web UI](../10-web-ui.md) und die Bildschirmfotos unter `docs/screenshots/`.

## 10. Stand, Grenzen und Roadmap

**Gebaut und in diesem Handbuch verifiziert (✅):** der lokale Kern mit allen oben genannten Befehlen; Pack-Segmente, Reflog, Signaturen, Submodule, git-Brücke; der Einzelknoten-Server mit CAS-Refs, Objektströmen, PRs, Policies, Leases, SSE, Webhooks, Root- und Bereichs-Tokens, Audit-Log, Schlüsselregister; die Wissensdatenbank mit BM25-Suche, Veralterung, Entwürfen, Kontextpaketen und lokalem Offline-Speicher; MCP-Server, Python- und TypeScript-SDK, Web-Oberfläche, Claude-Code-Hooks.

**Geplant (📋), mit Entwurf in den RFCs:** Journal mit Gruppen-Commit und speicherabgebildete Ref-Tabelle (Ziel über 10.000 Commits/s je Kern, RFC 0004); Raft-replizierter Commit-Log je Repository, Blob-Stufen mit S3-kompatiblem Kaltspeicher, Sharding, HTTP/2- und gRPC-Streaming, Idempotenzschlüssel; Notizobjekte im Repository auf `refs/knowledge/<branch>`, Symbol-Links, semantische Suche (RFC 0003); Squash- und Rebase-Merges, anwendbare Vorschläge, Entscheidungsprotokoll (RFC 0005); Server-seitiges Rendern und Symbolnavigation (RFC 0006); Submodul-Update-Strategien und repositoryübergreifende PR-Sätze (RFC 0007); Benutzer, Organisationen, OIDC; tokensparende Ausgaben (`diff --compact`, `status --short-json`); Rebase, Bisect; Format-Einfrieren und Spezifikation in Phase 5.

**Ehrliche Grenzen heute:** ein Server ohne Replikation (Backup des Verzeichnisses ist Pflicht); einige hundert Commits pro Sekunde je Prozess bei Standard-Dauerhaftigkeit; Wissen als serverseitiges JSON ohne Branching; Policies schützen vor Fehlern, nicht vor Angreifern, solange Identitäten nicht an Tokens gebunden sind; Tags werden nicht gepusht; Formate können sich vor 1.0 ändern.

**Beim Verifizieren gefundene Abweichungen** (vollständige Liste in [15. FAQ](../15-faq.md#known-discrepancies-and-rough-edges)), die wichtigsten:

1. `agit switch` schlägt in einem Repository, dessen Baum einen Submodul-Eintrag enthält, mit `io … Operation not permitted` fehl (`switch -c` am aktuellen Commit funktioniert); Klone solcher Repositories melden den Submodul-Pfad als geändert und verweigern `agit pull` mit `dirty_worktree`.
2. `agit merge --json` gibt bei Konflikten zwei JSON-Dokumente aus (Ergebnis, dann Fehlerumschlag).
3. `AGIT_JSON=1` wird abgelehnt; nur `AGIT_JSON=true` funktioniert.
4. `HEAD@{n}` aus `agit reflog` ist keine gültige Revision; Rückgängigmachen über die Id.
5. Non-Fast-Forward-Pushes melden `invalid` (Exit 1), nicht `ref_conflict` (Exit 75).
6. `/api/v1/health` ist ohne Token erreichbar, obwohl die Dokumentation das Gegenteil sagt.
7. An einen Agenten gebundene Tokens überschreiben den Header, aber nicht `holder` im Request-Body von `POST /leases`.
8. `GET /refs?prefix=refs/heads/main` (Präfix auf eine einzelne Ref) liefert einen `io`-Fehler; der Server versteht `~`/`^` nicht und Branch-Namen mit `/` müssen in Pfadsegmenten URL-kodiert werden.
9. Der Server akzeptiert Tags ohne Namespace; die Ausgabeformen von `agit know add|search|context` haben sich mit dem lokalen Speicher auf `{note|notes|context, source}` geändert.
10. Dokumentationsstand: README nennt Rust 1.80 (richtig: 1.89) und 32 MCP-Werkzeuge (richtig: 33); ARCHITECTURE.md beschreibt noch lose Objekte und `AGI1` statt Pack-Segmente und `AGI2`.

## 11. Lesereihenfolge

- **Entwickler:** [0](../00-introduction.md) → [3](../03-quickstart-developers.md) → [5](../05-concepts.md) → [6](../06-cli-reference.md) → [15](../15-faq.md).
- **Agentenautoren und Flottenbetreiber:** [1](../01-why.md) → [4](../04-quickstart-agents.md) → [7](../07-mcp-reference.md) → [9](../09-sdks.md) → [11](../11-knowledge-base.md) → [12](../12-multi-agent-workflows.md).
- **Operatoren:** [2](../02-installation.md) → [13](../13-server-administration.md) → [8](../08-http-api.md) → [10](../10-web-ui.md), dann [docs/DEPLOYMENT.md](../../DEPLOYMENT.md) und [docs/SECURITY.md](../../SECURITY.md).
- **Mitwirkende:** [5](../05-concepts.md) → [14](../14-architecture-internals.md) → [docs/rfcs/](../../rfcs) → [docs/ROADMAP.md](../../ROADMAP.md).

Ein Glossar aller Begriffe ist [16. Glossary](../16-glossary.md); der Vergleich mit git und GitHub-artigen Plattformen samt der Frage, wann man agit nicht einsetzen sollte, ist [17. Comparison](../17-comparison.md).

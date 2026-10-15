// Fake-server tests must not inherit AGIT_* defaults from the shell (e.g. a token meant
// for the live integration test). Call once at module top level.
const KEYS = ["AGIT_SERVER_URL", "AGIT_TOKEN", "AGIT_AGENT", "AGIT_MODEL"] as const;

export function isolateAgitEnv(): void {
  for (const k of KEYS) delete process.env[k];
}

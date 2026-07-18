// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const view = readFileSync(new URL("../views/project-center/CorrectionChecks.tsx", import.meta.url), "utf8");
const api = readFileSync(new URL("../api.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../../../../crates/hangar-api/src/controlled_checks.rs", import.meta.url), "utf8");
const tauri = readFileSync(new URL("../../src-tauri/src/main.rs", import.meta.url), "utf8");

describe("controlled correction checks contract", () => {
  it("keeps deterministic static checks explicitly non-executing", () => {
    expect(view).toContain("Executes no project code.");
    expect(backend).toContain("executed_project_code: false");
    const staticBody = backend.split("pub(crate) fn static_correction_check")[1]?.split("fn structural_check")[0] ?? "";
    for (const forbidden of ["Command::new", "run_bounded_process", "std::process"]) {
      expect(staticBody).not.toContain(forbidden);
    }
  });

  it("accepts only detected commands and never exposes a free command field", () => {
    for (const executable of ['"npm.cmd"', '"cargo.exe"', '"go.exe"', '"python.exe"']) {
      expect(backend).toContain(executable);
    }
    expect(backend).toContain("validate_check_identity(check_id, fingerprint)");
    expect(backend).toContain("spec.definition.fingerprint != fingerprint");
    expect(view).not.toContain('<input type="text"');
    expect(api).toContain("project_check_run");
    expect(api).not.toContain("project_check_run_shell");
  });

  it("requires manifest-bound approval with an honest side-effect disclosure", () => {
    expect(view).toContain("I understand this runs this project's code outside a sandbox");
    expect(view).toContain("Approve this exact check");
    expect(view).toContain("cannot promise to undo files or side effects");
    expect(view).toContain("I checked this command and want to run this exact project check now");
    expect(view).toContain("Run once");
    expect(view).toContain("confirmRunId !== definition.id || !runAcknowledged");
    expect(backend).toContain("project_check_approval(project_id, id)");
    expect(backend).toContain("stored == &fingerprint");
  });

  it("enforces the Windows resource boundary and bounded output in Rust", () => {
    for (const guard of [
      "CreateJobObjectW",
      "AssignProcessToJobObject",
      "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE",
      "JOB_OBJECT_LIMIT_ACTIVE_PROCESS",
      "JOB_OBJECT_LIMIT_JOB_MEMORY",
      "TerminateJobObject",
      ".env_clear()",
      'env("CARGO_NET_OFFLINE", "true")',
      'env("npm_config_offline", "true")',
      "OUTPUT_LIMIT_BYTES",
      "redact_secrets",
    ]) {
      expect(backend).toContain(guard);
    }
  });

  it("keeps execution commands out of the strict core feature surface", () => {
    expect(tauri).toMatch(
      /#\[cfg\(feature = "mutation"\)\]\s*#\[tauri::command\]\s*async fn project_check_run/,
    );
    expect(tauri).toContain("hangar_api::project_check_run");
  });
});

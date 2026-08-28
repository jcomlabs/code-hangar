use std::fs;
use std::path::Path;

pub fn write_markdown_fixture(root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(root.join("docs"))?;
    fs::write(root.join("README.md"), "# Fixture\n\nLocal context.")?;
    fs::write(root.join("AGENTS.md"), "# Instructions\n\nRead-only.")?;
    fs::write(
        root.join("docs").join("overview.md"),
        "# Overview\n\nScanner fixture.",
    )?;
    Ok(())
}

pub fn write_sensitive_fixture(root: &Path) -> std::io::Result<()> {
    fs::write(root.join("README.md"), "# Sensitive Fixture")?;
    fs::write(root.join(".env"), "SECRET=blocked")?;
    fs::write(root.join("credentials.json"), "{\"secret\":\"blocked\"}")?;
    Ok(())
}

pub fn write_codex_session_fixture(path: &Path) -> std::io::Result<()> {
    fs::write(
        path,
        concat!(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"Fix the fixture label"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"turn_diff","unified_diff":"diff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -2,2 +2,2 @@\n-old\n+new\n same"}}"#,
            "\n"
        ),
    )
}

pub fn write_claude_session_fixture(path: &Path) -> std::io::Result<()> {
    fs::write(
        path,
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"Update the app and notebook"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/app.ts","old_string":"false","new_string":"true"}},{"type":"tool_use","name":"Write","input":{"file_path":"src/new.ts","content":"export const ready = true;"}},{"type":"tool_use","name":"MultiEdit","input":{"file_path":"src/app.ts","edits":[{"old_string":"old","new_string":"new"}]}},{"type":"tool_use","name":"NotebookEdit","input":{"notebook_path":"analysis.ipynb","old_source":"print('old')","new_source":"print('new')"}}]}}"#,
            "\n"
        ),
    )
}

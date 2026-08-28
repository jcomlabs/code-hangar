# UI Principles

Code Hangar should feel calm, fast and precise.

## Three-pane layout

Left: Project Navigator.

Centre: File and Context Viewer.

Right: Inspector.

## Default project tab

Context, not Cleanup.

## Navigation feel

It should feel closer to Obsidian than to an IDE.

Clicking a project should show useful context immediately.

Clicking Markdown should preview immediately.

Opening a Markdown/text file from Windows should land in Files with that file
open inside its resolved project, not in a detached document window. A path
already contained by a known project opens there directly. An unknown file opens
directly in temporary Viewer mode: reading the requested file is the critical
path, while scanning/indexing runs behind it. The first preview must not wait for
the encrypted catalog to open and no new scan may be created before the document
has painted. Its provisional Viewer must be replaced by the resolved project
membership, never retained as a duplicate project, tab or Back-history entry.
This must never silently register the file's parent or aggregate other
projects/sessions. An unknown folder must
explain that Code Hangar does not yet know its project and offer:

- **Viewer:** the selected folder (or a file's parent) only, temporarily and
  read-only, with other projects and AI sessions kept out of the view;
- **Automatic:** show the detected ancestor root before registering/scanning it
  and correlating local AI-app sessions;
- **Manual:** let the user choose a containing project root, validate it, then
  scan from that root.

No unknown folder is silently registered merely because it came from Explorer.

The app is designed to stay ready: optional start-at-login opens it quietly in
the Windows notification area, closing the window returns it there, and the tray
provides Open, Refresh projects now and Exit. Background freshness must not steal
focus, block an open file, or claim that a correlated app caused an observed file
change.

## Context priority

Context files must appear before ordinary files:

- README.md
- AGENTS.md
- CLAUDE.md
- GEMINI.md
- .cursorrules
- .cursor/rules/*
- .clinerules
- docs/**/*.md
- prompts/**/*.md

## Wording

Do not overstate inferred relationships.

Use confidence-aware language.

High confidence: "This workflow references this model."

Medium confidence: "This workflow very likely references this model."

Low confidence: "This may be associated with this workflow. Review before acting."

Unknown: "Code Hangar cannot classify this relationship."

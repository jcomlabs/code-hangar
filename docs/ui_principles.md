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

import { memo } from "react";

import { HelpPopover } from "./ui";

export const BEGINNER_HELP = {
  whatChanged: {
    title: "What changed",
    paragraphs: [
      "This page collects clues about work done on the project. It combines changes recorded by AI conversations with differences Git can see in the project files.",
      "It is a review aid, not a perfect history. A tool can change a file without leaving a readable record. Opening this page never changes the project."
    ]
  },
  evidence: {
    title: "Local clues and evidence",
    paragraphs: [
      "Evidence means information Code Hangar found on this computer: AI conversation records, file differences and older review records.",
      "Complete means every checked source was readable. Some information missing means the visible changes are useful, but may not tell the whole story."
    ]
  },
  git: {
    title: "Git",
    paragraphs: [
      "Git keeps a local history of project files. A commit is a named snapshot. A branch is a separate line of work that can be reviewed before it is joined back together.",
      "Git is strongly recommended for AI-assisted projects because it helps you see and recover changes. Code Hangar only reads Git here; it does not commit, push or change branches."
    ]
  },
  lineChanges: {
    title: "Added and removed lines",
    paragraphs: [
      "A line comparison shows text before and after a change. Green + lines were added and red - lines were removed. Uncoloured lines are nearby text shown for context.",
      "A comparison tells you what changed, not whether the change is correct. Read the surrounding request and use the AI explanation or project checks when you need more confidence."
    ]
  },
  aiConversations: {
    title: "AI conversations",
    paragraphs: [
      "A conversation is a local record from a tool such as Claude, ChatGPT or Cursor. It can contain your request, the AI response and recorded file edits.",
      "A conversation is only linked to a project when Code Hangar finds a reliable local connection. Unlinked conversations remain in the Independent section."
    ]
  },
  reviewPoint: {
    title: "Last review point",
    paragraphs: [
      "Marking a review remembers where you stopped. Next time, Code Hangar can focus on newer conversations and file changes.",
      "It does not approve code, create a Git commit or change a project file."
    ]
  },
  context: {
    title: "Project context",
    paragraphs: [
      "Context files are the documents most likely to explain what a project is, how to run it and what rules an AI tool should follow.",
      "README, AGENTS and similar instruction files are shown first. They are normal local files; opening them is read-only."
    ]
  },
  fileTree: {
    title: "File tree",
    paragraphs: [
      "The file tree is the project's folder structure. Folders can be expanded and files can be opened in the centre without running the project.",
      "Generated dependencies and protected locations may be hidden or de-emphasised to keep the useful project files visible."
    ]
  },
  source: {
    title: "Rendered and source views",
    paragraphs: [
      "Rendered shows a document in a readable form. Source shows the exact text stored in the file, including code and formatting marks.",
      "Changing view does not edit the file. Editing controls remain separately locked."
    ]
  },
  space: {
    title: "Project space",
    paragraphs: [
      "Space is the amount of local disk storage used by files Code Hangar can connect to this project.",
      "Counts can be incomplete until a scan finishes. Shared files and caches need special handling so the same bytes are not counted twice."
    ]
  },
  connections: {
    title: "Project connections",
    paragraphs: [
      "Connections are local links Code Hangar found between files, workflows, models, caches and other projects.",
      "A connection is a clue, not permission to delete anything. Ambiguous or missing links are labelled for review."
    ]
  },
  models: {
    title: "Local model files",
    paragraphs: [
      "A model is a large local file containing learned AI data, such as a checkpoint, LoRA, VAE, GGUF or ONNX file. It is data used by an AI tool, not normal source code.",
      "Code Hangar infers the model category from its file type and folder. A model with no mapped workflow reference may still be used manually or by another tool, so it is only a review candidate."
    ]
  },
  workflows: {
    title: "AI workflows",
    paragraphs: [
      "A workflow is a local recipe that connects AI steps and may name the models it expects. Code Hangar reads supported bounded JSON fields to map those local references.",
      "A missing or ambiguous model name means the local recipe deserves review. Code Hangar does not run the workflow, download a model or repair the reference automatically."
    ]
  },
  caches: {
    title: "Dependency and tool caches",
    paragraphs: [
      "A cache is local material a tool keeps to avoid downloading, compiling or calculating the same data again. It can be large and can often be recreated, but that is not guaranteed.",
      "Caches may be shared by several projects or tools. Code Hangar groups them to reduce noise and keeps shared, protected or uncertain bytes out of simple recoverable totals."
    ]
  },
  sessions: {
    title: "Sessions",
    paragraphs: [
      "A session is one saved AI conversation. Project sessions are linked to a project; Independent sessions have no reliable project link yet.",
      "Opening a session reads its local record. Large conversations load gradually so the app stays responsive."
    ]
  },
  safeManage: {
    title: "Safe Manage",
    paragraphs: [
      "Safe Manage checks ownership, links, protection and possible consequences before any cleanup is considered.",
      "The first screens are review-only. Moving or removing files uses a separate guarded process with a verified backup and explicit confirmation."
    ]
  },
  scan: {
    title: "Scan",
    paragraphs: [
      "A scan reads local folder and file information so Code Hangar can build its map. It does not run the project or edit its files.",
      "Needs scan means the saved map may be old or incomplete. Empty means the selected project folder currently has no files to map."
    ]
  },
  inventory: {
    title: "Local inventory",
    paragraphs: [
      "The inventory is Code Hangar's private local map of projects, folders and file information. It is stored separately from the projects themselves.",
      "Removing inventory records does not delete project files. Disk cleanup is a different, separately confirmed action."
    ]
  },
  protected: {
    title: "Protected and sensitive files",
    paragraphs: [
      "Protected locations are areas Code Hangar refuses to inspect deeply or modify. Sensitive files may contain credentials, private keys or personal data.",
      "Protection is deliberately cautious. Temporarily revealing allowed content does not unlock editing or AI sending."
    ]
  },
  references: {
    title: "References",
    paragraphs: [
      "A reference is a local link from one file or project item to another. Dependents are items that appear to rely on the selected item.",
      "Missing or ambiguous references need human review. They do not prove that a file is safe or unsafe to remove."
    ]
  },
  recover: {
    title: "Recover",
    paragraphs: [
      "Recover shows files Code Hangar previously moved into its protected holding area and AI app listings that can be restored.",
      "Restore puts a verified copy back only after checking the destination. Existing files are never silently overwritten."
    ]
  },
  backup: {
    title: "Backup and held files",
    paragraphs: [
      "Before a guarded disk move, Code Hangar creates and verifies a separate backup. The held copy is the reversible copy kept after the move.",
      "A backup record is proof Code Hangar stored for recovery. It is not the same as Git and should not replace an independent backup strategy."
    ]
  },
  duplicates: {
    title: "Duplicate candidates",
    paragraphs: [
      "Duplicate candidates are files that may contain the same bytes. Code Hangar confirms complete file content before treating them as identical.",
      "Identical content does not automatically mean one copy is unnecessary. Location, ownership and project links still matter."
    ]
  },
  unreferenced: {
    title: "Unreferenced or forgotten items",
    paragraphs: [
      "Unreferenced means Code Hangar did not find a known local link to the item. Forgotten project means a project-like folder is no longer registered as an active root.",
      "Neither label proves that the item is unused. Treat it as a reason to investigate, never as permission to delete."
    ]
  },
  versions: {
    title: "Previous versions",
    paragraphs: [
      "A previous version is a verified local copy Code Hangar created before it changed one file.",
      "Restoring always shows the exact line comparison first and refuses to overwrite a file that changed unexpectedly."
    ]
  },
  projectChecks: {
    title: "Project checks",
    paragraphs: [
      "A project check runs a command supplied by the project, such as a test or validation command. Unlike normal browsing, it can execute project code.",
      "Code Hangar limits its resources and requires approval, but project code can still create side effects. Review the exact command every time."
    ]
  },
  localAutomation: {
    title: "Local automation",
    paragraphs: [
      "Local automation lets another program on this computer ask Code Hangar for specific information. The local endpoint is the private address it uses; a token is its one-time password.",
      "Permissions decide which projects and actions that program can request. Start with the smallest set, and revoke the program when it no longer needs access."
    ]
  }
} as const;

export type BeginnerHelpConcept = keyof typeof BEGINNER_HELP;

export const ConceptHelp = memo(function ConceptHelp({ concept }: { concept: BeginnerHelpConcept }) {
  const entry = BEGINNER_HELP[concept];
  return (
    <HelpPopover title={entry.title} compact>
      {entry.paragraphs.map((paragraph) => <p key={paragraph}>{paragraph}</p>)}
    </HelpPopover>
  );
});

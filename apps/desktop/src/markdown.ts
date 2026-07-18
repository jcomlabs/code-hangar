import type { MarkdownLink } from "./types";

export interface RenderedMarkdown {
  html: string;
  headings: string[];
  links: MarkdownLink[];
}

export interface MarkdownRenderOptions {
  localLinks?: "interactive" | "inert";
}

export function renderMarkdownSafe(markdown: string, options: MarkdownRenderOptions = {}): RenderedMarkdown {
  const headings: string[] = [];
  const links: MarkdownLink[] = [];
  const html: string[] = [];
  const paragraph: string[] = [];
  const localLinkMode = options.localLinks ?? "interactive";
  let inCode = false;
  let activeList: "ul" | "ol" | null = null;
  const lines = markdown.split(/\r?\n/);

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    html.push(`<p>${renderInline(paragraph.join(" "), links, localLinkMode)}</p>`);
    paragraph.length = 0;
  };

  const closeList = () => {
    if (!activeList) return;
    html.push(`</${activeList}>`);
    activeList = null;
  };

  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const rawLine = lines[lineIndex];
    const line = rawLine.trimEnd();
    const trimmed = line.trimStart();

    if (trimmed.startsWith("```")) {
      flushParagraph();
      closeList();
      html.push(inCode ? "</code></pre>" : "<pre><code>");
      inCode = !inCode;
      continue;
    }

    if (inCode) {
      html.push(`${escapeHtml(line)}\n`);
      continue;
    }

    const table = parseMarkdownTable(lines, lineIndex);
    if (table) {
      flushParagraph();
      closeList();
      html.push(renderTable(table.header, table.rows, links, localLinkMode));
      lineIndex = table.endIndex;
      continue;
    }

    if (trimmed.length === 0) {
      flushParagraph();
      closeList();
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(trimmed);
    if (heading) {
      flushParagraph();
      closeList();
      const level = heading[1].length;
      const text = heading[2].trim();
      headings.push(text);
      html.push(`<h${level}>${renderInline(text, links, localLinkMode)}</h${level}>`);
      continue;
    }

    const unorderedItem = /^[-+*]\s+(.+)$/.exec(trimmed);
    const orderedItem = /^\d+[.)]\s+(.+)$/.exec(trimmed);
    const listItem = unorderedItem ?? orderedItem;
    if (listItem) {
      flushParagraph();
      const nextList = unorderedItem ? "ul" : "ol";
      if (activeList !== nextList) {
        closeList();
        activeList = nextList;
        html.push(`<${nextList}>`);
      }
      html.push(`<li>${renderInline(listItem[1], links, localLinkMode)}</li>`);
      continue;
    }

    const quote = /^>\s?(.*)$/.exec(trimmed);
    if (quote) {
      flushParagraph();
      closeList();
      html.push(`<blockquote><p>${renderInline(quote[1], links, localLinkMode)}</p></blockquote>`);
      continue;
    }

    closeList();
    paragraph.push(trimmed);
  }

  flushParagraph();
  closeList();
  if (inCode) html.push("</code></pre>");

  return { html: html.join(""), headings, links };
}

export function markdownToPlainText(markdown: string): string {
  const generatedHtml = renderMarkdownSafe(markdown).html;
  const withoutTags = generatedHtml
    .replace(/<\/(?:h[1-6]|p|li|tr|th|td|pre|ul|ol|table|thead|tbody)>/g, " ")
    .replace(/<br\s*\/?>/g, " ")
    .replace(/<[^>]+>/g, "");
  return withoutTags
    .replaceAll("&quot;", '"')
    .replaceAll("&#39;", "'")
    .replaceAll("&#96;", "`")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&")
    .replace(/\s+/g, " ")
    .trim();
}

function renderInline(input: string, links: MarkdownLink[], localLinkMode: "interactive" | "inert"): string {
  let output = "";
  let index = 0;

  while (index < input.length) {
    if (input[index] === "`") {
      const end = input.indexOf("`", index + 1);
      if (end > index + 1) {
        output += `<code>${escapeHtml(input.slice(index + 1, end))}</code>`;
        index = end + 1;
        continue;
      }
    }

    const strongMarker = input.startsWith("**", index) ? "**" : input.startsWith("__", index) ? "__" : null;
    if (strongMarker) {
      const end = input.indexOf(strongMarker, index + strongMarker.length);
      if (end > index + strongMarker.length) {
        output += `<strong>${renderInline(input.slice(index + strongMarker.length, end), links, localLinkMode)}</strong>`;
        index = end + strongMarker.length;
        continue;
      }
    }

    if (input.startsWith("~~", index)) {
      const end = input.indexOf("~~", index + 2);
      if (end > index + 2) {
        output += `<del>${renderInline(input.slice(index + 2, end), links, localLinkMode)}</del>`;
        index = end + 2;
        continue;
      }
    }

    const image = parseTarget(input, index, true);
    if (image) {
      const remote = isRemoteTarget(image.target);
      links.push({ label: image.label, target: image.target, isRemote: remote });
      output += remote
        ? `<span class="preview-blocked-inline">Remote image blocked</span>`
        : `<span class="preview-local-image">Local image: ${escapeHtml(image.target)}</span>`;
      index = image.end;
      continue;
    }

    const link = parseTarget(input, index, false);
    if (link) {
      const remote = isRemoteTarget(link.target);
      links.push({ label: link.label, target: link.target, isRemote: remote });
      output += remote
        ? `<span class="preview-remote-link" data-help="Remote link disabled. Code Hangar keeps Markdown preview local-only and never opens remote URLs from preview.">${escapeHtml(link.label)}</span>`
        : localLinkMode === "inert"
          ? `<span class="preview-local-link-inert">${escapeHtml(link.label)}</span>`
          : `<a href="#" data-local-path="${escapeAttr(link.target)}">${escapeHtml(link.label)}</a>`;
      index = link.end;
      continue;
    }

    output += escapeHtml(input[index]);
    index += 1;
  }

  return output;
}

function parseMarkdownTable(lines: string[], startIndex: number): { header: string[]; rows: string[][]; endIndex: number } | null {
  const header = parseTableRow(lines[startIndex]);
  const separator = parseTableRow(lines[startIndex + 1] ?? "");
  if (!header || !separator || !isTableSeparator(separator)) return null;

  const rows: string[][] = [];
  let endIndex = startIndex + 1;
  for (let index = startIndex + 2; index < lines.length; index += 1) {
    const row = parseTableRow(lines[index]);
    if (!row) break;
    rows.push(row);
    endIndex = index;
  }
  return { header, rows, endIndex };
}

function parseTableRow(line: string): string[] | null {
  const trimmed = line.trim();
  if (!trimmed.includes("|")) return null;
  const body = trimmed.replace(/^\|/, "").replace(/\|$/, "");
  const cells = body.split("|").map((cell) => cell.trim());
  return cells.length >= 2 ? cells : null;
}

function isTableSeparator(cells: string[]): boolean {
  return cells.every((cell) => /^:?-{3,}:?$/.test(cell.replace(/\s+/g, "")));
}

function renderTable(
  header: string[],
  rows: string[][],
  links: MarkdownLink[],
  localLinkMode: "interactive" | "inert"
): string {
  const headerHtml = header.map((cell) => `<th>${renderInline(cell, links, localLinkMode)}</th>`).join("");
  const bodyHtml = rows
    .map((row) => {
      const normalized = header.map((_, index) => row[index] ?? "");
      return `<tr>${normalized.map((cell) => `<td>${renderInline(cell, links, localLinkMode)}</td>`).join("")}</tr>`;
    })
    .join("");
  return `<table><thead><tr>${headerHtml}</tr></thead><tbody>${bodyHtml}</tbody></table>`;
}

function parseTarget(input: string, index: number, image: boolean): { label: string; target: string; end: number } | null {
  const start = image ? "![" : "[";
  if (!input.startsWith(start, index)) return null;
  const labelStart = index + start.length;
  const labelEnd = input.indexOf("]", labelStart);
  if (labelEnd < 0 || input[labelEnd + 1] !== "(") return null;
  const targetStart = labelEnd + 2;
  const targetEnd = input.indexOf(")", targetStart);
  if (targetEnd < 0) return null;
  return {
    label: input.slice(labelStart, labelEnd),
    target: input.slice(targetStart, targetEnd),
    end: targetEnd + 1
  };
}

function isRemoteTarget(target: string): boolean {
  const lower = target.trim().toLowerCase();
  return lower.startsWith("http://") || lower.startsWith("https://") || lower.startsWith("//") || lower.startsWith("data:");
}

export function escapeHtml(input: string): string {
  return input
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function escapeAttr(input: string): string {
  return escapeHtml(input).replaceAll("`", "&#96;");
}

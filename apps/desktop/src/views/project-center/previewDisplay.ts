import type { FilePreview } from "../../types";

export function shouldUseReadableSourcePreview(preview: Pick<FilePreview, "mode" | "fileKind" | "source" | "renderedHtml"> & Partial<Pick<FilePreview, "displayName" | "displayPath" | "path">>) {
  if (preview.mode !== "rendered" || !((preview.source ?? preview.renderedHtml)?.trim())) return false;
  const name = (preview.displayName || preview.displayPath || preview.path || "").toLocaleLowerCase();
  if (preview.fileKind === "markdown" || /\.(md|mdx|markdown)$/.test(name)) return false;
  if (preview.fileKind === "text") return true;
  return isTechnicalTextPreviewName(name);
}

function isTechnicalTextPreviewName(name: string) {
  const baseName = name.split(/[\\/]/).filter(Boolean).at(-1) ?? name;
  if (["package.json", "package-lock.json", "tsconfig.json", "jsconfig.json", ".env", ".env.example", ".gitignore"].includes(baseName)) return true;
  return /\.(json|jsonc|toml|ya?ml|lock|txt|log|csv|tsv|js|jsx|ts|tsx|mjs|cjs|rs|py|ps1|sh|bat|cmd|css|scss|sql)$/.test(baseName);
}

export function readableSourcePreviewText(preview: Pick<FilePreview, "source" | "renderedHtml"> & Partial<Pick<FilePreview, "displayName" | "displayPath" | "path">>) {
  const text = preview.source ?? renderedHtmlToText(preview.renderedHtml ?? "");
  return formatStructuredPreviewText(text, preview.displayName || preview.displayPath || preview.path || "");
}

function renderedHtmlToText(html: string) {
  return decodeBasicHtmlEntities(
    html
      .replace(/<br\s*\/?>/gi, "\n")
      .replace(/<\/p>\s*<p[^>]*>/gi, "\n\n")
      .replace(/<[^>]*>/g, "")
  );
}

function decodeBasicHtmlEntities(value: string) {
  return value
    .replace(/&quot;/g, "\"")
    .replace(/&#39;/g, "'")
    .replace(/&#x27;/gi, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

function formatStructuredPreviewText(text: string, name: string) {
  const baseName = name.toLocaleLowerCase().split(/[\\/]/).filter(Boolean).at(-1) ?? name.toLocaleLowerCase();
  if (/(\.json|\.jsonc)$/.test(baseName) || ["package.json", "package-lock.json", "tsconfig.json", "jsconfig.json"].includes(baseName)) {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }
  return text;
}

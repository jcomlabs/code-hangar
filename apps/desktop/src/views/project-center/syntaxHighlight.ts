export type SyntaxTokenKind = "plain" | "comment" | "string" | "number" | "keyword" | "literal" | "operator";

export interface SyntaxToken {
  kind: SyntaxTokenKind;
  text: string;
}

export interface HighlightedSource {
  language: string;
  tokens: SyntaxToken[];
  limited: boolean;
}

const MAX_HIGHLIGHT_CHARS = 240_000;

const LANGUAGE_KEYWORDS: Record<string, Set<string>> = {
  javascript: new Set(["async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else", "export", "extends", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "of", "return", "static", "switch", "throw", "try", "typeof", "var", "while", "with", "yield"]),
  typescript: new Set(["abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "declare", "default", "delete", "do", "else", "enum", "export", "extends", "finally", "for", "from", "function", "if", "implements", "import", "in", "infer", "instanceof", "interface", "keyof", "let", "namespace", "new", "of", "private", "protected", "public", "readonly", "return", "satisfies", "static", "switch", "throw", "try", "type", "typeof", "var", "while", "yield"]),
  rust: new Set(["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while"]),
  python: new Set(["and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield"]),
  powershell: new Set(["begin", "break", "catch", "class", "continue", "data", "do", "dynamicparam", "else", "elseif", "end", "enum", "exit", "filter", "finally", "for", "foreach", "from", "function", "hidden", "if", "in", "param", "process", "return", "switch", "throw", "trap", "try", "until", "using", "var", "while"]),
  shell: new Set(["case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "return", "then", "until", "while"]),
  sql: new Set(["alter", "and", "as", "asc", "begin", "by", "case", "create", "delete", "desc", "distinct", "drop", "else", "end", "from", "group", "having", "in", "index", "insert", "into", "is", "join", "limit", "not", "null", "on", "or", "order", "select", "set", "table", "then", "union", "update", "values", "when", "where"]),
  css: new Set(["@container", "@font-face", "@import", "@keyframes", "@media", "@supports", "from", "important", "to"])
};

const LITERALS = new Set(["false", "null", "true", "undefined", "None", "True", "False"]);

export function sourceLanguage(path: string): string {
  const name = path.toLowerCase().split(/[\\/]/).filter(Boolean).at(-1) ?? path.toLowerCase();
  if (/\.(tsx?|mts|cts)$/.test(name) || name.includes("tsconfig")) return "typescript";
  if (/\.(jsx?|mjs|cjs)$/.test(name)) return "javascript";
  if (name.endsWith(".rs")) return "rust";
  if (name.endsWith(".py")) return "python";
  if (name.endsWith(".ps1")) return "powershell";
  if (/\.(sh|bash|zsh)$/.test(name)) return "shell";
  if (name.endsWith(".sql")) return "sql";
  if (/\.(css|scss)$/.test(name)) return "css";
  if (/\.(json|jsonc)$/.test(name) || ["package-lock.json", "package.json"].includes(name)) return "json";
  if (name.endsWith(".toml")) return "toml";
  if (/\.ya?ml$/.test(name)) return "yaml";
  if (/\.(md|mdx|markdown)$/.test(name)) return "markdown";
  return "plain";
}

export function highlightSource(source: string, path: string): HighlightedSource {
  const language = sourceLanguage(path);
  if (source.length > MAX_HIGHLIGHT_CHARS || language === "plain" || language === "markdown") {
    return { language, tokens: [{ kind: "plain", text: source }], limited: source.length > MAX_HIGHLIGHT_CHARS };
  }
  const tokens: SyntaxToken[] = [];
  const keywords = LANGUAGE_KEYWORDS[language] ?? new Set<string>();
  const hashComments = ["python", "powershell", "shell", "toml", "yaml"].includes(language);
  const slashComments = ["javascript", "typescript", "rust", "json"].includes(language);
  const blockComments = ["javascript", "typescript", "rust", "json", "css"].includes(language);
  const sqlComments = language === "sql";
  let index = 0;

  const push = (kind: SyntaxTokenKind, text: string) => {
    if (!text) return;
    const previous = tokens.at(-1);
    if (previous?.kind === kind) previous.text += text;
    else tokens.push({ kind, text });
  };

  while (index < source.length) {
    const char = source[index];
    const next = source[index + 1];
    if ((hashComments && char === "#") || (slashComments && char === "/" && next === "/") || (sqlComments && char === "-" && next === "-")) {
      const end = source.indexOf("\n", index);
      const stop = end === -1 ? source.length : end;
      push("comment", source.slice(index, stop));
      index = stop;
      continue;
    }
    if (blockComments && char === "/" && next === "*") {
      const end = source.indexOf("*/", index + 2);
      const stop = end === -1 ? source.length : end + 2;
      push("comment", source.slice(index, stop));
      index = stop;
      continue;
    }
    if (char === "\"" || char === "'" || ((language === "javascript" || language === "typescript") && char === "`")) {
      const quote = char;
      let stop = index + 1;
      while (stop < source.length) {
        if (source[stop] === "\\") {
          stop += 2;
          continue;
        }
        if (source[stop] === quote) {
          stop += 1;
          break;
        }
        stop += 1;
      }
      push("string", source.slice(index, stop));
      index = stop;
      continue;
    }
    if (/\d/.test(char) && (index === 0 || !/[\w$]/.test(source[index - 1]))) {
      const match = source.slice(index).match(/^(?:0x[\da-f]+|0b[01]+|\d+(?:\.\d+)?(?:e[+-]?\d+)?)/i);
      const text = match?.[0] ?? char;
      push("number", text);
      index += text.length;
      continue;
    }
    if (/[A-Za-z_$@]/.test(char)) {
      const match = source.slice(index).match(/^[A-Za-z_$@][\w$@-]*/);
      const text = match?.[0] ?? char;
      const normalized = language === "sql" ? text.toLowerCase() : text;
      push(LITERALS.has(text) ? "literal" : keywords.has(normalized) ? "keyword" : "plain", text);
      index += text.length;
      continue;
    }
    if (/[{}()[\].,;:+*/%<>=!&|?~-]/.test(char)) {
      push("operator", char);
      index += 1;
      continue;
    }
    push("plain", char);
    index += 1;
  }

  return { language, tokens, limited: false };
}

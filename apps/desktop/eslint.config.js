import reactHooks from "eslint-plugin-react-hooks";
import tsParser from "@typescript-eslint/parser";

// Minimal, focused lint: the React Hooks rules only. exhaustive-deps catches the exact class
// of bug that hid the AI-Assist "Explain this code" menu (a useCallback that read connectorBuild
// but omitted it from the deps array → stale closure). rules-of-hooks catches conditional hooks.
// We deliberately do NOT enable the full typescript-eslint rule set here — tsc --noEmit already
// type-checks; this config exists only to enforce hook-dependency correctness.
export default [
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
        ecmaFeatures: { jsx: true }
      }
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      // "error" (not "warn") so a NEW missing/stale hook dependency FAILS local-ci — the exact
      // class that hid the AI-Assist menu. The few genuinely-intentional effects carry an inline
      // eslint-disable-next-line with a justification.
      "react-hooks/exhaustive-deps": "error"
    }
  }
];

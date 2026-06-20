import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactPlugin from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  // Ignore build output and generated files
  {
    ignores: ["dist/**", "src-tauri/**", "node_modules/**", "*.config.js"],
  },

  // JavaScript base rules
  js.configs.recommended,

  // TypeScript recommended rules (no type-aware rules, to avoid over-configuration)
  ...tseslint.configs.recommended,

  // React rules
  {
    plugins: {
      react: reactPlugin,
      "react-hooks": reactHooks,
    },
    settings: {
      react: { version: "detect" },
    },
    rules: {
      // React Hooks rules: violations cause runtime bugs, must stay enabled
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",

      // React 17+ JSX transform, no manual React import needed
      "react/react-in-jsx-scope": "off",
      "react/prop-types": "off",

      // TypeScript tweak: existing code has some `any`; warn instead of error for now
      "@typescript-eslint/no-explicit-any": "warn",
      // Allow unused variables that follow the _-prefix convention
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],

      // Flag leftover console.log (warn level; clean up before release)
      "no-console": ["warn", { allow: ["warn", "error"] }],
    },
  },

  // Disable formatting rules that conflict with Prettier (must come last)
  prettier,
);

// @ts-check
import eslint from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default tseslint.config(
  eslint.configs.recommended,
  tseslint.configs.recommended,
  reactHooks.configs["recommended-latest"],
  reactRefresh.configs.vite,
  {
    languageOptions: {
      globals: globals.browser,
    },
  },
  {
    ignores: ["dist/", "coverage/", "node_modules/"],
  },
);

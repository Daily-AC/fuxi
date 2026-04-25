// flat config 给 eslint 9
import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import solidPlugin from "eslint-plugin-solid";
import globals from "globals";

export default [
  {
    ignores: [
      "dist/**",
      "dev-dist/**",
      "node_modules/**",
      "playwright-report/**",
      "test-results/**",
      "coverage/**",
    ],
  },
  {
    files: ["**/*.{ts,tsx,js,cjs}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: { ecmaVersion: "latest", sourceType: "module", ecmaFeatures: { jsx: true } },
      globals: { ...globals.browser, ...globals.node, ...globals.serviceworker },
    },
    plugins: { "@typescript-eslint": tsPlugin, solid: solidPlugin },
    rules: {
      "no-console": ["warn", { allow: ["warn", "error"] }],
      "no-unused-vars": "off",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", disallowTypeAnnotations: false },
      ],
      "solid/reactivity": "warn",
      "solid/no-destructure": "warn",
      "solid/jsx-no-undef": "error",
      "solid/components-return-once": "warn",
    },
  },
  {
    files: ["tests/**/*.{ts,tsx}"],
    rules: {
      "no-console": "off",
    },
  },
];

// Lint typé (type-aware) : attrape les vraies erreurs de comportement — promesses
// non gérées, await sur non-thenable, variables mortes, comparaisons douteuses —
// sans se noyer dans le bruit des casts DOM (unsafe-*), assumés dans ce front.
import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**", "*.config.js"] },
  js.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  {
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
      globals: {
        window: "readonly",
        document: "readonly",
        navigator: "readonly",
        localStorage: "readonly",
        console: "readonly",
        setTimeout: "readonly",
        clearTimeout: "readonly",
        requestAnimationFrame: "readonly",
        WebSocket: "readonly",
        ResizeObserver: "readonly",
        HTMLElement: "readonly",
        HTMLInputElement: "readonly",
        HTMLCanvasElement: "readonly",
        HTMLFormElement: "readonly",
        MouseEvent: "readonly",
        KeyboardEvent: "readonly",
        DragEvent: "readonly",
        TextEncoder: "readonly",
        TextDecoder: "readonly",
        ImageData: "readonly",
        Uint8Array: "readonly",
        Uint8ClampedArray: "readonly",
        DataView: "readonly",
        ArrayBuffer: "readonly",
        alert: "readonly",
        confirm: "readonly",
        prompt: "readonly",
        atob: "readonly",
        btoa: "readonly",
        getComputedStyle: "readonly",
      },
    },
    rules: {
      // Le cœur du lint « comportement » : ce sont ces règles qui auraient
      // attrapé nos bugs (promesses oubliées, handlers async mal branchés).
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": ["error", { "checksVoidReturn": false }],
      "@typescript-eslint/await-thenable": "error",
      "no-console": ["warn", { allow: ["warn", "error"] }],
      // Bruit assumé : le front manipule le DOM avec des casts explicites.
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-return": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-non-null-assertion": "off",
      // Faux positifs avec le helper générique $<T>() : retirer le cast casse tsc.
      "@typescript-eslint/no-unnecessary-type-assertion": "off",
      "@typescript-eslint/restrict-template-expressions": "off",
      "@typescript-eslint/no-confusing-void-expression": "off",
    },
  },
);

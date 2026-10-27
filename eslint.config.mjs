import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import tseslint from "typescript-eslint";

export default defineConfig([
  // Vendored third-party code, copied byte for byte by extension/build.mjs.
  globalIgnores(["extension/dist/vendor/"]),
  {
    files: ["protocol/ts/**/*.ts", "extension/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, tseslint.configs.recommended],
  },
]);

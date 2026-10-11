import { build } from "esbuild";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
await build({ entryPoints: [`${root}js/rich-editor.js`], outfile: `${root}editor-assets/rich-editor.js`, bundle: true, format: "iife", target: "es2020", minify: true });

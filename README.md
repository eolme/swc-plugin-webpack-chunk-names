# swc-plugin-webpack-chunk-names

An SWC plugin that automatically injects `/* webpackMode: "lazy-once" */` and `/* webpackChunkName: "…" */` into dynamic `import()` calls, generating deterministic chunk names from import paths.

Comments are emitted as **leading trivia on the specifier** (the string or template literal), i.e. inside the `import(…)` parentheses and immediately before the module path—not before the `import` keyword. Existing `webpackChunkName` / `webpackMode` block comments at that position are replaced; other leading block comments are kept.

**Before:**

```js
const Button = lazy(() => import("./components/Button"));
```

**After:**

```js
const Button = lazy(() =>
  import(
    /* webpackMode: "lazy-once" */ /* webpackChunkName: "mfWidgets.Button" */ "./components/Button"
  )
);
```

Exact spacing after emit may depend on your printer/SWC pipeline; both magic comments are always attached to the specifier.

## Installation

```bash
npm install swc-plugin-webpack-chunk-names
```

Add to your SWC config (`.swcrc`):

```json
{
  "jsc": {
    "experimental": {
      "plugins": [
        ["swc-plugin-webpack-chunk-names", {}]
      ]
    }
  }
}
```

## Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `dirs` | `string[]` | `["src", "lib", "dist"]` | Source directories used to identify package boundaries when resolving chunk name prefixes. |

Example with custom dirs:

```json
["swc-plugin-webpack-chunk-names", { "dirs": ["client", "shared"] }]
```

## Chunk Name Format

The general format is **`camelCasePrefix.leaf`** where:

- **Prefix** identifies the package or module owner, converted to camelCase (`-` and `_` are treated as word separators).
- **Leaf** is the last path segment with file extensions stripped (`.tsx`, `.ts`, `.js`, etc.).

Special cases:

- A trailing `/` appends `.index` (e.g. `lodash/` becomes `lodash.index`).
- A bare package with no sub-path produces just the camelCased prefix (e.g. `lodash` stays `lodash`).
- Absolute paths (`/...`) are rejected.

## Chunk Name Reference

### Bare Imports

Standard npm package imports. The first path segment is used as the prefix unless a source directory is found deeper in the path (see [Source-Dir Aware Resolution](#source-dir-aware-resolution)).

| Import Path | Chunk Name |
|---|---|
| `lodash` | `lodash` |
| `my-package` | `myPackage` |
| `lodash/fp/get` | `lodash.get` |
| `lodash.get` | `lodash.get` |
| `lodash/index` | `lodash.index` |
| `lodash/` | `lodash.index` |
| `lodash/fp/` | `lodash.fp` |
| `punycode` | `punycode` |
| `punycode/` | `punycode.index` |
| `lib/lib/test-package/src/Component` | `testPackage.Component` |

### Scoped Imports

For `@scope/pkg/...` imports, the prefix is formed by joining `scope` and `pkg` with `-`, then camelCasing the result.

| Import Path | Chunk Name |
|---|---|
| `@scope/pkg` | `scopePkg` |
| `@my-scope/my-pkg` | `myScopeMyPkg` |
| `@scope/pkg/sub/Leaf` | `scopePkg.Leaf` |
| `@my-company/package-name/src/my/test/ComponentName` | `myCompanyPackageName.ComponentName` |
| `@my-company/package-name/my/test/ComponentName` | `myCompanyPackageName.ComponentName` |
| `@scope-a/ui/Button.tsx` | `scopeAUi.Button` |
| `@scope-b/ui/Button.tsx` | `scopeBUi.Button` |
| `@scope/my-lib/utils/helpers.ts` | `scopeMyLib.helpers` |
| `@scope/pkg/` | `scopePkg.index` |
| `@scope/pkg/sub/` | `scopePkg.sub` |

### `@/` Alias Imports

Paths starting with `@/` are treated as bare imports after stripping the `@/` prefix. Source-dir aware resolution applies.

| Import Path | Chunk Name |
|---|---|
| `@/components/Button` | `components.Button` |
| `@/utils/helpers/format` | `utils.format` |
| `@/lib` | `lib` |
| `@/lib/` | `lib.index` |
| `@/lib/test-package/src/Component` | `testPackage.Component` |

### Relative Imports

For `./` and `../` imports, the plugin resolves the path against the current file's directory, then finds the owning package by scanning for a configured source directory.

| Import Path | Current File | Chunk Name |
|---|---|---|
| `./components/MyWidget` | `/project/mf-widgets/src/App.tsx` | `mfWidgets.MyWidget` |
| `./Button.tsx` | `/project/mf-widgets/src/components/App.tsx` | `mfWidgets.Button` |
| `../GroupB/components/GroupBChild` | `/project/mf-widgets/src/components/GroupA/index.tsx` | `mfWidgets.GroupBChild` |
| `../../GroupTitle` | `/project/mf-widgets/src/components/Group/components/GroupName/index.tsx` | `mfWidgets.GroupTitle` |
| `./components/` | `/project/mf-widgets/src/App.tsx` | `mfWidgets.components` |
| `./components/Button` | `/project/node_modules/@scope1/pkg1/node_modules/@scope2/pkg2/src/index.tsx` | `pkg2.Button` |
| `../utils/formatDate` | `/project/node_modules/@company/ui-kit/src/components/DatePicker.tsx` | `uiKit.formatDate` |
| `./locale/en` | `/project/node_modules/some-lib/src/index.tsx` | `someLib.en` |

### Template Literal Imports

All dynamic expressions in template literals are replaced with a fixed `dynamic` placeholder, regardless of the expression type. This ensures stable and predictable chunk names.

| Import Expression | Flattened Path | Chunk Name (relative context) |
|---|---|---|
| `` `./locale/${language}` `` | `./locale/dynamic` | `mfWidgets.dynamic` |
| `` `./v${2}/api` `` | `./vdynamic/api` | *(depends on context)* |
| `` `./${scope}/${name}` `` | `./dynamic/dynamic` | *(depends on context)* |
| `` `./locale/${lang.code}` `` | `./locale/dynamic` | *(depends on context)* |
| `` `./locale/${getLocale()}` `` | `./locale/dynamic` | *(depends on context)* |

## Source-Dir Aware Resolution

When resolving the package prefix for bare and `@/` alias imports, the plugin scans the path segments left-to-right looking for configured source directories (`src`, `lib`, `dist` by default). The segment immediately **before** the rightmost source-dir match is used as the package prefix.

If no source directory is found, the first path segment is used as fallback.

**Example:** For import path `lib/lib/test-package/src/Component` with default dirs:

```
lib / lib / test-package / src / Component
                 ^          ^
            prefix found   source dir match (rightmost)
```

Result: `testPackage.Component`

For relative imports, the same logic applies to the resolved absolute path. The segment before the rightmost source directory in the resolved path identifies the package, and the import's last segment becomes the leaf.

**Example:** File `/project/mf-widgets/src/App.tsx` importing `./components/Button`:

```
project / mf-widgets / src / components / Button
              ^          ^
         prefix found   source dir match
```

Result: `mfWidgets.Button`

## Development

- **Tests:** `cargo test` — unit tests and `test_inline!` integration-style cases live in [`src/tests.rs`](src/tests.rs).
- **Wasm artifact for npm:** run [`build.sh`](./build.sh) (release build for `wasm32-wasip1`, then `wasm-opt` into `lib/swc_plugin_webpack_chunk_names.wasm`). Requires a nightly-capable Rust toolchain and the WASI target as set up in your environment.

## License

MIT

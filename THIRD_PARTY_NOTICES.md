# Third-party notices

Code Hangar's original source is licensed under Apache-2.0. Third-party
components remain under their own licenses and copyright notices.

## Tauri

The desktop application is built with Tauri and Tauri plugins, licensed under
Apache-2.0 OR MIT by their respective contributors.

Repository: <https://github.com/tauri-apps/tauri>

## React and Lucide

The UI uses React/React DOM (MIT) and Lucide React (ISC). Copyright belongs to
their respective contributors.

Repositories: <https://github.com/facebook/react>,
<https://github.com/lucide-icons/lucide>

## SQLite, SQLCipher, and rusqlite

The encrypted catalog uses rusqlite's bundled SQLCipher build. rusqlite is MIT
licensed. SQLite and SQLCipher source included by the build remain under their
respective upstream licenses and notices.

Repositories: <https://github.com/rusqlite/rusqlite>,
<https://github.com/sqlcipher/sqlcipher>

## Microsoft WebView2 Runtime

Windows installers include the offline Microsoft Edge WebView2 Runtime so the
Local edition can install without downloading a bootstrapper. The runtime is
Microsoft redistributable material governed by Microsoft's WebView2 terms and
is not covered by Code Hangar's Apache-2.0 license.

Documentation: <https://developer.microsoft.com/microsoft-edge/webview2/>

## Remaining dependencies

The full exact dependency graphs and versions are recorded in `Cargo.lock` and
`package-lock.json`. Their upstream license files and notices continue to apply.
See [`SOURCES.md`](SOURCES.md) for the directly incorporated dependency families.

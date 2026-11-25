# Third-party notices

The desktop shell includes code and styles under the following MIT license.
A readable copy is also shipped as `licenses/shell-MIT.txt`.

MIT License

Copyright (c) 2026 Thomas Gamble

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## YAML editing

The form uses yaml 2.9.1 under its ISC license. Its upstream notice is shipped
as `licenses/yaml-ISC.txt` in the same app artifact directories.

## JavaScript bridge

The Tauri JavaScript API is distributed under its MIT license option.
Its complete notice is shipped as `licenses/Tauri-API-MIT.txt`.

## Native folder picker

The folder picker uses tauri-plugin-dialog 2.8.4, tauri-plugin-fs 2.7.1 and
rfd 0.17.2 under their MIT license options. Their complete upstream notices
are shipped as `licenses/tauri-plugin-dialog-MIT.txt`,
`licenses/tauri-plugin-fs-MIT.txt` and `licenses/rfd-MIT.txt`.

## Embedded Git

The app includes git2 0.21.0 and libgit2-sys 0.18.8+1.9.7 under their MIT
license option (`licenses/git2-MIT.txt`). Vendored libgit2 1.9.7 is distributed
under GPL-2.0 with its linking exception; its complete upstream license and
bundled third-party notices are in `licenses/libgit2-COPYING.txt`.

The Git dependency includes libz-sys 1.1.32 under its MIT license option
(`licenses/libz-sys-MIT.txt`) and may bundle zlib under its zlib license
(`licenses/zlib-LICENSE.txt`). The fixed Git index checksum uses sha1 0.10.6
under its MIT license option (`licenses/sha1-MIT.txt`). These notices are
copied from the published crates and included in app artifacts.

## Kubernetes access

The app uses kube, kube-client and kube-core 5.0.0 and k8s-openapi 0.29.0
under Apache-2.0. Complete upstream licenses are shipped as
`licenses/kube-Apache-2.0.txt` and `licenses/k8s-openapi-Apache-2.0.txt`.
The kube license is copied from its published source revision; the other
licenses are copied from the published crates.

HTTP/authentication support uses http 1.5.0 and tower 0.5.3 under their MIT
license options (`licenses/http-MIT.txt`, `licenses/tower-MIT.txt`). Base64 encoding
uses its MIT license option (`licenses/base64-MIT.txt`). These notices are
included in the same app artifact license directories as existing notices.

Pod-stream HTTP uses hyper 1.12.0, hyper-util 0.1.21 and http-body-util 0.1.5
under their MIT licenses. Upstream notices are shipped as
`licenses/hyper-MIT.txt`, `licenses/hyper-util-MIT.txt` and
`licenses/http-body-util-MIT.txt` in the same app artifact directories.

## Temporary pack copies

The app uses tempfile 3.27.0 and fastrand 2.5.0 under their MIT license options.
Their upstream notices are shipped as `licenses/tempfile-MIT.txt` and
`licenses/fastrand-MIT.txt`.

## Fonts

Geist and Geist Mono are bundled under the SIL Open Font License 1.1. Their
complete notices are in `app/public/licenses/` and are included in app
artifacts. The existing Inter and JetBrains Mono notices remain preserved there.
These licenses do not replace this project's Microsoft MIT license.

The app's Microsoft MIT notice is shipped as `licenses/Orka-MIT.txt`.
Third-party trademarks remain subject to their owners' policies.

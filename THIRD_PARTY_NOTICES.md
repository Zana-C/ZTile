# Third-party notices

ZTile's own code is licensed under the MIT License (see `LICENSE`).

## Compiled into ztile.exe

The executable contains code from Microsoft's Windows crates for Rust
(<https://github.com/microsoft/windows-rs>), used under the MIT License
(they are dual-licensed MIT OR Apache-2.0):

`windows`, `windows-core`, `windows-collections`, `windows-future`,
`windows-link`, `windows-numerics`, `windows-result`, `windows-strings`,
`windows-threading`

```
MIT License

Copyright (c) Microsoft Corporation.

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
```

## Build-time only (not part of ztile.exe)

Used by the Windows crates' procedural macros while compiling; nothing from them
ends up in the executable: `syn`, `quote`, `proc-macro2` (MIT OR Apache-2.0),
`unicode-ident` ((MIT OR Apache-2.0) AND Unicode-3.0).

## Trademarks

Windows is a trademark of the Microsoft group of companies. Other product names
mentioned in this project (for example in the documentation) belong to their
respective owners. ZTile is an independent project and is not affiliated with or
endorsed by any of them.

## Compiled into the Linux program (`ztile`)

The Linux program is built from crates.io packages under permissive licenses
(versions pinned in `linux/Cargo.lock`):

| License | Crates |
|---|---|
| MIT | wayland-client, wayland-backend, wayland-protocols, wayland-scanner, wayland-sys, quick-xml, float-cmp, imagesize, pico-args, rgb, simd-adler32, strict-num |
| MIT OR Apache-2.0 | dbus, libdbus-sys, libc, png, resvg, usvg, kurbo, simplecss, svgtypes, roxmltree, data-url, euclid, arrayvec, bitflags, bytemuck, cfg-if, crc32fast, downcast-rs, errno, fdeflate, flate2, log, miniz_oxide, num-traits, polycool, siphasher, smallvec, adler2, memchr, rustix, linux-raw-sys, unicode-ident, proc-macro2, quote, windows-link, windows-sys |
| BSD-3-Clause | tiny-skia, tiny-skia-path (notice below) |
| BSD-2-Clause | arrayref (notice below) |
| Zlib | zlib-rs (notice below) |

It links at run time to the system's libdbus (AFL-2.1 or GPL-2.0) and talks to
KDE Plasma's KWin over D-Bus; neither is included.

### tiny-skia, tiny-skia-path

```
Copyright (c) 2011 Google Inc. All rights reserved.
Copyright (c) 2020 Yevhenii Reizner All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

  * Redistributions of source code must retain the above copyright
    notice, this list of conditions and the following disclaimer.

  * Redistributions in binary form must reproduce the above copyright
    notice, this list of conditions and the following disclaimer in
    the documentation and/or other materials provided with the
    distribution.

  * Neither the name of the copyright holder nor the names of its
    contributors may be used to endorse or promote products derived
    from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### arrayref

```
Copyright (c) 2015 David Roundy <roundyd@physics.oregonstate.edu>
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the
   distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### zlib-rs

```
(C) 2024 Trifecta Tech Foundation 

This software is provided 'as-is', without any express or implied
warranty. In no event will the authors be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.

3. This notice may not be removed or altered from any source distribution.
```

## Used by the Linux settings window (not included in this repository)

The settings window (`linux/ui/`) is Python source and bundles no third-party
code. While it's open it uses the system's Python, **PyQt6** (Riverbank
Computing, GPL v3 or commercial license) and Qt 6 (LGPL v3), installed from your
distribution and not redistributed by ZTile. If you ship ZTile together with
PyQt6 (e.g. in a bundle or package that includes it), the combined work falls
under the GPL v3; ZTile's MIT-licensed code is compatible with that.

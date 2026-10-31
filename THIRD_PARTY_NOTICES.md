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

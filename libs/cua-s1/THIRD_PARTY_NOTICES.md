# Third-party notices

This Cua-S1 component does not bundle third-party model weights, datasets,
binaries, or media.

## jevlike

Parts of `python/src/cua_s1/model.py` are adapted from the MIT-licensed
[`jevlike`](https://github.com/vinnylarouge/jevlike) project at commit
[`94f5fd1b0b11d52bbdfdf4e0ee6aa96b568f8452`](https://github.com/vinnylarouge/jevlike/commit/94f5fd1b0b11d52bbdfdf4e0ee6aa96b568f8452).
The adapted material is limited to the byte-collation and small attention-model
primitives in that file.

Copyright (c) 2026 Minimal Labs

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The Python package declares Hatchling as a build-system requirement. Hatchling
is build tooling and is not bundled in the resulting package; it is distributed
under its own terms.

The Python package declares its runtime dependencies in `python/pyproject.toml`.
The component-local `python/uv.lock` records the tested development resolution.
Those dependencies remain distributed under their own licenses and are not
copied into this repository.

Future checkpoint or software distributions must update this file with the
applicable notices for all included third-party code, models, datasets, assets,
and other materials. A reference to Cua-S1 or `cua-s1-form-v0` does not grant
rights to third-party material.

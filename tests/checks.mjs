// Runtime-agnostic contract checks for the npm package. Each runtime harness
// (smoke.mjs for Node/Bun/Deno, browser.html for Chromium) loads the API and
// fixture bytes its own way, then calls runChecks. Expected values are the
// same ground truth the Rust unit tests assert
// (`src/{pkdrawing,crl,decode}.rs`).

const near = (a, b, eps = 0.02) => Math.abs(a - b) <= eps;

function assert(cond, message) {
  if (!cond) throw new Error(`check failed: ${message}`);
}

function assertEq(actual, expected, message) {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`check failed: ${message}: ${a} !== ${b}`);
}

/**
 * @param api  the `libfreeform` module namespace
 * @param fixtures  { inkPen, nativeMixed, tsuDescription, realBoardNative, realBoardTsu } as Uint8Array
 * @returns number of checks run
 */
export function runChecks(api, fixtures) {
  const { inkPen, nativeMixed, tsuDescription, realBoardNative, realBoardTsu } = fixtures;
  let count = 0;
  const check = (name, fn) => {
    try {
      fn();
    } catch (err) {
      throw new Error(`[${name}] ${err.message}`, { cause: err });
    }
    count += 1;
  };

  check('classifyBlob routes by filename and signature', () => {
    assertEq(
      api.classifyBlob('com_apple_freeform_CRLNativeData', new Uint8Array()),
      'crlNative',
      'crl name',
    );
    assertEq(
      api.classifyBlob('com_apple_freeform_TSUDescription', new Uint8Array()),
      'tsuDescription',
      'tsu name',
    );
    assertEq(api.classifyBlob('public_png', new Uint8Array()), 'renderPng', 'png name');
    assertEq(api.classifyBlob('blob.bin', inkPen), 'drawing', 'wrd magic');
    assertEq(api.classifyBlob('blob.bin', tsuDescription), 'tsuDescription', 'bare bplist');
    assertEq(
      api.classifyBlob('notes.txt', new TextEncoder().encode('hello')),
      undefined,
      'unrelated',
    );
  });

  check('isPkDrawing checks the wrd magic', () => {
    assert(api.isPkDrawing(inkPen), 'ink fixture is a PKDrawing');
    assert(!api.isPkDrawing(tsuDescription), 'bplist is not a PKDrawing');
  });

  check('decodePkDrawing recovers strokes, transforms, pressure, ink', () => {
    const drawing = api.decodePkDrawing(inkPen);
    assertEq(
      drawing.strokes.map(s => s.points.length),
      [4, 2],
      'stroke point counts',
    );
    const [s0] = drawing.strokes;
    assert(near(s0.points[0].x, 100) && near(s0.points[0].y, 50), 'transform applied to p0');
    assert(near(s0.points[3].x, 130) && near(s0.points[3].y, 60), 'transform applied to p3');
    assertEq(
      s0.points.map(p => Math.round(p.force * 10) / 10),
      [0.2, 0.5, 0.8, 0.4],
      'forces',
    );
    assertEq(s0.color, '#ff4245', 'srgb ink color');
    assertEq(s0.inkType, 'pen', 'ink family');
    assertEq(s0.opacity, 1, 'ink opacity');
  });

  check('decodePkDrawing throws a typed error on garbage', () => {
    let threw;
    try {
      api.decodePkDrawing(new Uint8Array([1, 2, 3, 4]));
    } catch (err) {
      threw = err;
    }
    assert(
      threw instanceof Error && threw.message.includes('wrd'),
      `typed decode error, got ${threw}`,
    );
  });

  check('decodeCrlNative joins the TSU manifest in order', () => {
    const native = api.decodeCrlNative(nativeMixed, tsuDescription);
    assertEq(native.pasteId, '12345678-90AB-CDEF-1234-567890ABCDEF', 'paste id');
    assertEq(
      native.items.map(i => i.className),
      ['CRLWPStickyNoteItem', 'CRLWPShapeItem', 'CRLConnectionLineItem'],
      'classes',
    );
    assertEq(native.items[1].hints.textbox, 'true', 'textbox hint');
    assert(native.texts.includes('hi') && native.texts.includes('aaa'), 'tswp text');
    assert(!native.texts.includes('commonCRDTData'), 'crdt noise dropped');
    for (const hex of ['#e54d99', '#54bef0']) {
      assert(native.colors.includes(hex), `color ${hex}`);
    }
  });

  check('parseTsuDescription separates classes from hints', () => {
    const entries = api.parseTsuDescription(tsuDescription);
    assertEq(
      entries.map(e => e.className),
      ['CRLWPStickyNoteItem', 'CRLWPShapeItem', 'CRLConnectionLineItem'],
      'classes',
    );
    assert(!('class' in entries[1].hints), 'class key stripped from hints');
  });

  check('decodePasteboard assembles present flavors and passes renderPng through', () => {
    const renderPng = new Uint8Array([1, 2, 3]);
    const decoded = api.decodePasteboard({
      drawing: inkPen,
      crlNative: nativeMixed,
      tsuDescription,
      renderPng,
    });
    assertEq(decoded.drawing.strokes.length, 2, 'drawing tier');
    assertEq(decoded.native.items.length, 3, 'native tier');
    assert(decoded.renderPng === renderPng, 'renderPng passthrough is identity');
  });

  check('decodePasteboard degrades damaged tiers to missing', () => {
    const decoded = api.decodePasteboard({
      drawing: new TextEncoder().encode('wrd\xff\xff'),
      crlNative: new TextEncoder().encode('garbage'),
    });
    assert(decoded.drawing === undefined, 'bad drawing degrades');
    assert(decoded.native === undefined, 'bad native degrades');
    assert(decoded.renderPng === undefined, 'no renderPng invented');
  });

  check('hasFreeformContent requires a carrying flavor', () => {
    assert(!api.hasFreeformContent({}), 'empty');
    assert(api.hasFreeformContent({ drawing: inkPen }), 'pkdrawing counts');
    assert(!api.hasFreeformContent({ drawing: tsuDescription }), 'non-wrd drawing does not');
    assert(api.hasFreeformContent({ crlNative: new Uint8Array([0]) }), 'any crlNative counts');
  });

  check('real 10-item board decodes geometry, fills, and text', () => {
    const board = api.decodeCrlNative(realBoardNative, realBoardTsu);
    assertEq(board.items.length, 10, 'item count');

    const line = board.items[1];
    assert(near(line.frame.x, 320.4769) && near(line.frame.w, 212.132), 'line frame');
    assert(near(line.frame.rotation, -Math.PI / 4), 'line rotation');

    const sticky = board.items[7];
    assertEq(sticky.className, 'CRLWPStickyNoteItem', 'sticky class');
    assertEq(sticky.fill, '#ffe16c', 'sticky fill');
    assertEq(sticky.text, 'hi', 'sticky text');
    assert(near(sticky.frame.x, 1159) && near(sticky.frame.w, 200), 'sticky frame');

    const table = board.items[8];
    assertEq(table.fill, '#bfbfbf', 'table grid fill');
    assertEq(table.text, 'aaa\naaa\naaa', 'table cell text');

    const image = board.items[9];
    assertEq(image.className, 'CRLImageItem', 'image class');
    assert(near(image.frame.w, 250) && near(image.frame.h, 187.5), 'image frame');
  });

  return count;
}

// Runtime-agnostic contract checks for the published npm API. Harnesses supply
// the same fixture bytes in Node, Bun, and Deno.

function assert(condition, message) {
  if (!condition) throw new Error(`check failed: ${message}`);
}

function assertEq(actual, expected, message) {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) throw new Error(`check failed: ${message}: ${left} !== ${right}`);
}

function decoded(tier, name) {
  assert(tier.status === 'decoded', `${name} decoded, got ${tier.status}`);
  return tier.value;
}

function failed(tier, name) {
  assert(tier.status === 'failed', `${name} failed independently, got ${tier.status}`);
  assert(
    typeof tier.value.kind === 'string' && typeof tier.value.message === 'string',
    `${name} structured failure`,
  );
}

/**
 * Exercise the public JavaScript contract.
 * @param api the `libfreeform` module namespace
 * @param fixtures fixture bytes supplied by the runtime harness
 * @returns number of checks run
 */
export function runChecks(api, fixtures) {
  const { inkPen, tsuDescription, realBoardNative, realBoardTsu } = fixtures;
  let count = 0;
  const check = (name, fn) => {
    try {
      fn();
    } catch (error) {
      throw new Error(`[${name}] ${error.message}`, { cause: error });
    }
    count += 1;
  };

  check('classification only inspects names and prefixes', () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
    assertEq(api.classifyBlob('com.apple.freeform.CRLNativeData'), 'crlNative', 'native UTI');
    assertEq(
      api.classifyBlob('com.apple.freeform.TSUDescription'),
      'tsuDescription',
      'manifest UTI',
    );
    assertEq(api.classifyBlob('public.png', png), 'renderPng', 'render UTI and signature');
    assertEq(
      api.classifyBlob('com.apple.freeform.CRLNativeMetadata'),
      'nativeMetadata',
      'metadata UTI',
    );
    assertEq(api.classifyBlob('com.apple.freeform.CRLAsset.placeholder'), 'asset', 'asset UTI');
    assertEq(
      api.classifyBlob('com.apple.freeform.pasteboardState.CRLNativeData'),
      'state',
      'native name inside a state flag is not native data',
    );
    assertEq(api.classifyBlob('unrelated_CRLNativeData.bin'), undefined, 'unrelated dump name');
    assertEq(
      api.classifyBlob('com.apple.freeform.pasteboardState.selection'),
      'state',
      'state UTI',
    );
    assertEq(api.classifyBlob('blob.bin', inkPen), 'drawing', 'drawing prefix');
    assertEq(
      api.classifyBlob('blob.bin', tsuDescription),
      undefined,
      'generic bplist is not assumed TSU',
    );
    assertEq(
      api.classifyBlob('notes.txt', new TextEncoder().encode('hello')),
      undefined,
      'unrelated input',
    );
    assert(api.isPkDrawing(inkPen), 'PKDrawing magic');
    assert(!api.isPkDrawing(tsuDescription), 'bplist is not PKDrawing');
  });

  check('direct decoders expose lossless typed fields and errors', () => {
    const drawing = api.decodePkDrawing(inkPen);
    assertEq(
      drawing.strokes.map(stroke => stroke.points.length),
      [4, 2],
      'stroke order',
    );
    assert(typeof drawing.strokes[0].inkIdentifier === 'string', 'exact ink identifier');
    assert(drawing.strokes[0].transform != null, 'stroke transform retained');
    assert(drawing.strokes[0].rawData instanceof Uint8Array, 'stroke raw bytes retained');
    let error;
    try {
      api.decodePkDrawing(new Uint8Array([1, 2, 3]));
    } catch (caught) {
      error = caught;
    }
    assert(
      error != null && typeof error.kind === 'string' && typeof error.message === 'string',
      'structured decode error',
    );
  });

  check('TSU manifests preserve recursive typed values', () => {
    const entries = api.parseTsuDescription(tsuDescription);
    assertEq(
      entries.map(entry => entry.className),
      ['CRLWPStickyNoteItem', 'CRLWPShapeItem', 'CRLConnectionLineItem'],
      'class order',
    );
    const values = Object.values(entries[1].hints);
    assert(
      values.every(value => value != null && typeof value.kind === 'string'),
      'typed recursive hints',
    );
  });

  check(
    'exact snapshot preserves tiers, render bytes, metadata, assets, state, and diagnostics',
    () => {
      const render = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
      const state = new Uint8Array([9, 8]);
      const decodedPasteboard = api.decodePasteboard({
        changeCount: 42,
        flavors: [
          { uti: 'com.apple.drawing', bytes: inkPen },
          { uti: 'com.apple.freeform.CRLNativeData', bytes: realBoardNative },
          { uti: 'com.apple.freeform.TSUDescription', bytes: realBoardTsu },
          { uti: 'public.png', bytes: render },
          { uti: 'com.apple.freeform.CRLNativeMetadata', bytes: new Uint8Array() },
          { uti: 'com.apple.freeform.CRLAsset.placeholder', bytes: new Uint8Array() },
          { uti: 'com.apple.freeform.pasteboardState.selection', bytes: state },
          { uti: 'com.example.unknown', bytes: new Uint8Array([7]) },
        ],
      });
      assertEq(decoded(decodedPasteboard.drawing, 'drawing').strokes.length, 2, 'drawing content');
      assertEq(decoded(decodedPasteboard.native, 'native').items.length, 10, 'native content');
      assertEq(
        [...decodedPasteboard.renders[0].bytes],
        [...render],
        'render bytes retained in pasteboard order',
      );
      assert(
        decodedPasteboard.metadata.status !== 'absent',
        'provided metadata reports a tier outcome',
      );
      assert(typeof decodedPasteboard.assets === 'object', 'asset map');
      assert(
        Object.values(decodedPasteboard.state).some(
          bytes => JSON.stringify([...bytes]) === JSON.stringify([...state]),
        ),
        'state bytes retained',
      );
      assert(
        decodedPasteboard.unknownFlavors.some(flavor => flavor.uti === 'com.example.unknown'),
        'unknown flavor retained',
      );
      assert(Array.isArray(decodedPasteboard.diagnostics), 'diagnostics retained');
    },
  );

  check('tiers fail independently rather than disappearing', () => {
    const result = api.decodePasteboard({
      flavors: [
        { uti: 'com.apple.drawing', bytes: new TextEncoder().encode('wrd\xff\xff') },
        { uti: 'com.apple.freeform.CRLNativeData', bytes: realBoardNative },
        { uti: 'com.apple.freeform.TSUDescription', bytes: realBoardTsu },
      ],
    });
    failed(result.drawing, 'drawing');
    assertEq(decoded(result.native, 'native').items.length, 10, 'native survives drawing failure');
    assertEq(decoded(result.manifest, 'manifest').length, 10, 'manifest survives drawing failure');
  });

  check('TSU-only and render-only snapshots remain meaningful', () => {
    const tsuOnly = api.decodePasteboard({
      flavors: [{ uti: 'com.apple.freeform.TSUDescription', bytes: tsuDescription }],
    });
    assertEq(decoded(tsuOnly.manifest, 'TSU-only manifest').length, 3, 'TSU-only decode');
    assert(
      tsuOnly.native.status === 'absent' && tsuOnly.drawing.status === 'absent',
      'unprovided tiers absent',
    );
    const render = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
    const renderOnly = api.decodePasteboard({ flavors: [{ uti: 'public.png', bytes: render }] });
    assertEq([...renderOnly.renders[0].bytes], [...render], 'render-only bytes retained');
    assert(renderOnly.manifest.status === 'absent', 'render does not fabricate manifest');
  });

  check('rich item variants carry geometry, style, and tagged data', () => {
    const native = api.decodeCrlNative(realBoardNative, realBoardTsu);
    assertEq(
      native.compatibility,
      { kind: 'supported', version: [4, 4] },
      'nested-object compatibility',
    );
    assertEq(native.items.length, 10, 'board item count');
    const sticky = native.items[7];
    assertEq(sticky.kind.text.plain, 'hi', 'native sticky text');
    assertEq(sticky.style.fill.color.hex, '#FFE16C', 'native sticky color');
    assertEq(native.items[0].kind.kind, 'textBox', 'textbox hint refines native shape');
    const table = native.items[8];
    assertEq(table.kind.columnWidths, [344, 344], 'native table columns');
    assertEq(
      table.kind.cells.filter(cell => cell.text?.plain === 'aaa').length,
      3,
      'native cell text',
    );
    assertEq(table.children[0].tableAnchor.geometry.frame.x, 97, 'cell-local anchored shape');
    assert(
      table.kind.cells.some(cell => cell.anchoredItemIds.includes(table.children[0].uuid)),
      'cell references nested shape',
    );
    const image = native.items[9];
    assert(image.kind.assetId != null, 'native image asset reference');
    assert(native.assets[image.kind.assetId] != null, 'native asset descriptor');
  });

  check('content detection uses exact snapshot flavors', () => {
    assert(!api.hasFreeformContent({ flavors: [] }), 'empty snapshot');
    assert(
      api.hasFreeformContent({ flavors: [{ uti: 'com.apple.drawing', bytes: inkPen }] }),
      'drawing snapshot',
    );
    assert(
      !api.hasFreeformContent({ flavors: [{ uti: 'com.apple.drawing', bytes: tsuDescription }] }),
      'non-PK drawing',
    );
    assert(
      !api.hasFreeformContent({
        flavors: [{ uti: 'com.apple.freeform.CRLNativeData', bytes: new Uint8Array() }],
      }),
      'empty native data is not importable',
    );
  });

  check(
    'numbered native transport retains original flavors and restores the complete graph',
    () => {
      const flavors = [
        { uti: 'com.apple.freeform.CRLNativeData.1', bytes: realBoardNative.slice(1024) },
        { uti: 'com.apple.freeform.CRLNativeData', bytes: realBoardNative.slice(0, 1024) },
      ];
      const snapshot = api.decodePasteboard({ flavors });
      assertEq(
        decoded(snapshot.native, 'chunked native').items[7].kind.text.plain,
        'hi',
        'reassembled content',
      );
      assertEq(
        snapshot.flavors.map(flavor => flavor.uti),
        flavors.map(flavor => flavor.uti),
        'capture order',
      );
      assert(
        snapshot.flavors.every(flavor => flavor.bytes instanceof Uint8Array),
        'raw chunk bytes',
      );
    },
  );

  return count;
}

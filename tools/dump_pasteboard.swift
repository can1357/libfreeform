// dump_pasteboard.swift — enumerate the macOS pasteboard and dump every flavor to disk.
//
//   swift dump_pasteboard.swift [OUTDIR]
//
// Writes one file per pasteboard type (reverse-DNS dots -> underscores) into OUTDIR
// (default: ./pb_dump) and prints a size table. Use this right after a Freeform copy.
// See ../docs/FORMAT.md for what each `com.apple.freeform.*` type means.

import AppKit
import Foundation

let outDir = CommandLine.arguments.dropFirst().first ?? "./pb_dump"
try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

let pb = NSPasteboard.general
print("changeCount:", pb.changeCount)
for t in pb.types ?? [] {
    let data = pb.data(forType: t)
    let n = data?.count ?? 0
    print(String(format: "%12d  %@", n, t.rawValue))
    if let data = data {
        let name = t.rawValue.replacingOccurrences(of: ".", with: "_")
                             .replacingOccurrences(of: " ", with: "_")
        try? data.write(to: URL(fileURLWithPath: "\(outDir)/\(name)"))
    }
}
print("\nwrote flavors to \(outDir)/")

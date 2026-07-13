// dump_pasteboard.swift — capture every macOS pasteboard flavor as one atomic snapshot.
//
//   swift dump_pasteboard.swift [OUTDIR]
//
// The destination is a directory containing manifest.json and one opaque file per
// pasteboard type. The manifest is the authoritative ordered UTI-to-file mapping.
//
// The capture is retried if the pasteboard changes while it is being read. A
// completed destination therefore represents one pasteboard changeCount, never a
// mixture of clipboard states.

import AppKit
import Darwin
import Foundation

private let maximumCaptureAttempts = 3

private struct FlavorManifest: Codable {
    let index: Int
    let uti: String
    let file: String
    let byteCount: Int
}

private struct SnapshotManifest: Codable {
    let formatVersion: Int
    let initialChangeCount: Int
    let finalChangeCount: Int
    let flavors: [FlavorManifest]
}

private struct CapturedSnapshot {
    let directory: URL
    let manifest: SnapshotManifest
}

private enum CaptureError: LocalizedError {
    case usage
    case missingData(String)
    case changedDuringCapture(initial: Int, observed: Int)
    case unstablePasteboard(attempts: Int, lastInitial: Int?, lastObserved: Int?)

    var errorDescription: String? {
        switch self {
        case .usage:
            return "usage: swift dump_pasteboard.swift [OUTDIR]"
        case .missingData(let uti):
            return "pasteboard flavor \(uti) was advertised but did not provide byte data"
        case .changedDuringCapture(let initial, let observed):
            return "pasteboard changed during capture (changeCount \(initial) -> \(observed))"
        case .unstablePasteboard(let attempts, let initial, let observed):
            if let initial, let observed {
                return "pasteboard changed during all \(attempts) capture attempts (last changeCount \(initial) -> \(observed))"
            }
            return "pasteboard did not remain stable for \(attempts) capture attempts"
        }
    }
}

private func temporaryDirectory(for destination: URL, fileManager: FileManager) throws -> URL {
    let parent = destination.deletingLastPathComponent()
    try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)

    let name = ".\(destination.lastPathComponent).capture-\(UUID().uuidString)"
    let directory = parent.appendingPathComponent(name, isDirectory: true)
    try fileManager.createDirectory(at: directory, withIntermediateDirectories: false)
    return directory
}

private func removeDirectory(_ directory: URL, fileManager: FileManager) throws {
    try fileManager.removeItem(at: directory)
}

private func captureAttempt(
    pasteboard: NSPasteboard,
    destination: URL,
    fileManager: FileManager
) throws -> CapturedSnapshot {
    let stagingDirectory = try temporaryDirectory(for: destination, fileManager: fileManager)

    do {
        let initialChangeCount = pasteboard.changeCount
        let types = pasteboard.types ?? []
        var flavors: [FlavorManifest] = []
        flavors.reserveCapacity(types.count)

        for (index, type) in types.enumerated() {
            let beforeRead = pasteboard.changeCount
            guard beforeRead == initialChangeCount else {
                throw CaptureError.changedDuringCapture(initial: initialChangeCount, observed: beforeRead)
            }

            let data = pasteboard.data(forType: type)

            let afterRead = pasteboard.changeCount
            guard afterRead == initialChangeCount else {
                throw CaptureError.changedDuringCapture(initial: initialChangeCount, observed: afterRead)
            }

            guard let data else {
                throw CaptureError.missingData(type.rawValue)
            }

            // The ordinal is collision-safe even when UTI strings contain the same
            // characters after filesystem normalization.
            let filename = String(format: "flavor-%06d.bin", index)
            try data.write(
                to: stagingDirectory.appendingPathComponent(filename),
                options: .atomic
            )
            flavors.append(
                FlavorManifest(
                    index: index,
                    uti: type.rawValue,
                    file: filename,
                    byteCount: data.count
                )
            )
        }

        let finalChangeCount = pasteboard.changeCount
        guard finalChangeCount == initialChangeCount else {
            throw CaptureError.changedDuringCapture(initial: initialChangeCount, observed: finalChangeCount)
        }

        let manifest = SnapshotManifest(
            formatVersion: 1,
            initialChangeCount: initialChangeCount,
            finalChangeCount: finalChangeCount,
            flavors: flavors
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(manifest).write(
            to: stagingDirectory.appendingPathComponent("manifest.json"),
            options: .atomic
        )

        let afterManifest = pasteboard.changeCount
        guard afterManifest == initialChangeCount else {
            throw CaptureError.changedDuringCapture(initial: initialChangeCount, observed: afterManifest)
        }

        return CapturedSnapshot(directory: stagingDirectory, manifest: manifest)
    } catch {
        try removeDirectory(stagingDirectory, fileManager: fileManager)
        throw error
    }
}

private func publish(_ snapshot: CapturedSnapshot, to destination: URL, fileManager: FileManager) throws {
    do {
        if fileManager.fileExists(atPath: destination.path) {
            // replaceItemAt performs a same-volume replacement; staging lives beside
            // the destination, so readers see either the old complete snapshot or
            // the new complete snapshot, never its individual flavor files.
            _ = try fileManager.replaceItemAt(
                destination,
                withItemAt: snapshot.directory,
                backupItemName: nil,
                options: []
            )
        } else {
            try fileManager.moveItem(at: snapshot.directory, to: destination)
        }
    } catch {
        try removeDirectory(snapshot.directory, fileManager: fileManager)
        throw error
    }
}

private func captureAndPublish(to destination: URL, fileManager: FileManager) throws -> SnapshotManifest {
    var lastInitial: Int?
    var lastObserved: Int?

    for _ in 0..<maximumCaptureAttempts {
        do {
            let snapshot = try captureAttempt(
                pasteboard: .general,
                destination: destination,
                fileManager: fileManager
            )
            try publish(snapshot, to: destination, fileManager: fileManager)
            return snapshot.manifest
        } catch CaptureError.changedDuringCapture(let initial, let observed) {
            lastInitial = initial
            lastObserved = observed
        }
    }

    throw CaptureError.unstablePasteboard(
        attempts: maximumCaptureAttempts,
        lastInitial: lastInitial,
        lastObserved: lastObserved
    )
}

private func destinationURL(argument: String) -> URL {
    URL(
        fileURLWithPath: argument,
        relativeTo: URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true)
    ).standardizedFileURL
}

private func reportError(_ error: Error) {
    let message = "dump_pasteboard: \(error.localizedDescription)\n"
    FileHandle.standardError.write(Data(message.utf8))
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard arguments.count <= 1 else {
    reportError(CaptureError.usage)
    exit(EXIT_FAILURE)
}

let destination = destinationURL(argument: arguments.first ?? "./pb_dump")

do {
    let manifest = try captureAndPublish(to: destination, fileManager: .default)
    print("changeCount: \(manifest.initialChangeCount)")
    for flavor in manifest.flavors {
        print(String(format: "%12d  %@", flavor.byteCount, flavor.uti))
    }
    print("\nwrote \(manifest.flavors.count) flavors to \(destination.path)/")
} catch {
    reportError(error)
    exit(EXIT_FAILURE)
}

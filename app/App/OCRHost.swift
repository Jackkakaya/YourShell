import Foundation
import UIKit
import Vision

/// Native OCR backing the shell's `ocr` command, powered by the Vision
/// framework (on-device, offline). Called from the Rust core with an
/// absolute image path; returns recognized text (caller frees via
/// ys_ocr_free) or an "ERROR: ..." string.
@_cdecl("ys_ocr_run")
public func ys_ocr_run(_ path: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    let file = String(cString: path)
    guard let cgImage = UIImage(contentsOfFile: file)?.cgImage else {
        return strdup("ERROR: cannot load image: \(file)")
    }

    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false

    let handler = VNImageRequestHandler(cgImage: cgImage)
    do {
        try handler.perform([request])
    } catch {
        return strdup("ERROR: vision: \(error.localizedDescription)")
    }

    let lines = (request.results ?? []).compactMap {
        $0.topCandidates(1).first?.string
    }
    return strdup(lines.joined(separator: "\n"))
}

@_cdecl("ys_ocr_free")
public func ys_ocr_free(_ s: UnsafeMutablePointer<CChar>?) {
    free(s)
}

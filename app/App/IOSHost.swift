import Foundation
import UIKit

private func mainSync<T>(_ body: () -> T) -> T {
    if Thread.isMainThread { return body() }
    return DispatchQueue.main.sync(execute: body)
}

func iosClipboardCopy(_ bytes: UnsafePointer<UInt8>?, _ len: Int) -> Int32 {
    guard let bytes, len >= 0 else { return 1 }
    let data = Data(bytes: bytes, count: len)
    mainSync {
        UIPasteboard.general.setValue(String(decoding: data, as: UTF8.self),
                                      forPasteboardType: "public.utf8-plain-text")
    }
    return 0
}

func iosClipboardPaste(
    _ ctx: UnsafeMutableRawPointer?,
    _ output: (@convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void)?
) -> Int32 {
    guard let output else { return 1 }
    let text: String? = mainSync { UIPasteboard.general.string }
    guard let data = text?.data(using: .utf8) else { return 0 }
    data.withUnsafeBytes { raw in
        output(ctx, raw.bindMemory(to: UInt8.self).baseAddress, data.count)
    }
    return 0
}

func iosOpen(_ bytes: UnsafePointer<UInt8>?, _ len: Int) -> Int32 {
    guard let bytes, len >= 0 else { return 2 }
    let text = String(decoding: Data(bytes: bytes, count: len), as: UTF8.self)
    guard let url = URL(string: text) else { return 2 }
    mainSync {
        UIApplication.shared.open(url, options: [:], completionHandler: nil)
    }
    return 0
}

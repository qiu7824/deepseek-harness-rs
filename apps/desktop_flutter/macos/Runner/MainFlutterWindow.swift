import Cocoa
import FlutterMacOS
import ImageIO

class MainFlutterWindow: NSWindow {
  private var clipboardChannel: FlutterMethodChannel?

  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    registerClipboardChannel(flutterViewController.engine.binaryMessenger)

    super.awakeFromNib()
  }

  /// Exposes copied files and images that Flutter's text-only clipboard API
  /// cannot read. Files take precedence over image content.
  private func registerClipboardChannel(_ messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "dsh/clipboard", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      guard call.method == "read" else {
        result(FlutterMethodNotImplemented)
        return
      }
      let pasteboard = NSPasteboard.general
      var payload: [String: Any] = [:]
      let urls = pasteboard.readObjects(
        forClasses: [NSURL.self],
        options: [.urlReadingFileURLsOnly: true]
      ) as? [URL] ?? []
      let files = urls.map { $0.path }
      guard files.count <= 8 else {
        result(FlutterError(code: "clipboard-too-many-files", message: "Clipboard contains more than eight files", details: nil))
        return
      }
      payload["files"] = files
      do {
        if files.isEmpty, let png = try MainFlutterWindow.clipboardPng(pasteboard) {
          payload["png"] = FlutterStandardTypedData(bytes: png)
        }
      } catch ClipboardReadError.tooLarge {
        result(FlutterError(code: "clipboard-too-large", message: "Clipboard image exceeds the size limit", details: nil))
        return
      } catch {
        result(FlutterError(code: "clipboard-invalid-image", message: "Clipboard image could not be decoded", details: nil))
        return
      }
      result(payload)
    }
    clipboardChannel = channel
  }

  private enum ClipboardReadError: Error { case tooLarge, invalidImage }

  private static func imageSource(_ data: Data) throws -> CGImageSource {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil),
      let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [String: Any],
      let width = properties[kCGImagePropertyPixelWidth as String] as? Int,
      let height = properties[kCGImagePropertyPixelHeight as String] as? Int,
      width > 0, height > 0
    else { throw ClipboardReadError.invalidImage }
    guard width <= 32000000 / height else { throw ClipboardReadError.tooLarge }
    return source
  }

  private static func clipboardPng(_ pasteboard: NSPasteboard) throws -> Data? {
    if let png = pasteboard.data(forType: .png) {
      guard png.count <= 16 * 1024 * 1024 else { throw ClipboardReadError.tooLarge }
      _ = try imageSource(png)
      return png
    }
    guard let tiff = pasteboard.data(forType: .tiff) else { return nil }
    let source = try imageSource(tiff)
    guard let decoded = CGImageSourceCreateImageAtIndex(source, 0, nil),
      let png = NSBitmapImageRep(cgImage: decoded).representation(using: .png, properties: [:])
    else { throw ClipboardReadError.invalidImage }
    guard png.count <= 16 * 1024 * 1024 else { throw ClipboardReadError.tooLarge }
    return png
  }
}

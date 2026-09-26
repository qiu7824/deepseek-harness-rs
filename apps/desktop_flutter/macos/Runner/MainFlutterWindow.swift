import Cocoa
import FlutterMacOS

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
      payload["files"] = files
      if files.isEmpty, let png = MainFlutterWindow.clipboardPng(pasteboard) {
        payload["png"] = FlutterStandardTypedData(bytes: png)
      }
      result(payload)
    }
    clipboardChannel = channel
  }

  private static func clipboardPng(_ pasteboard: NSPasteboard) -> Data? {
    if let png = pasteboard.data(forType: .png) {
      return png
    }
    guard let tiff = pasteboard.data(forType: .tiff),
      let image = NSBitmapImageRep(data: tiff)
    else {
      return nil
    }
    return image.representation(using: .png, properties: [:])
  }
}

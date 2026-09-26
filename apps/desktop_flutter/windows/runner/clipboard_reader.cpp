#include "clipboard_reader.h"

#include <shellapi.h>
#include <wincodec.h>
#include <wrl/client.h>

#include <algorithm>
#include <array>

namespace {
using Microsoft::WRL::ComPtr;
constexpr size_t kMaxBytes = 16 * 1024 * 1024;
constexpr uint64_t kMaxPixels = 32000000;
constexpr UINT kMaxFiles = 8;

class ClipboardLock {
 public:
  explicit ClipboardLock(HWND owner) : opened_(OpenClipboard(owner) != FALSE) {}
  ~ClipboardLock() { if (opened_) CloseClipboard(); }
  bool opened() const { return opened_; }
 private:
  bool opened_;
};

std::string ToUtf8(const std::wstring& text) {
  const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS,
      text.data(), static_cast<int>(text.size()), nullptr, 0, nullptr, nullptr);
  if (size <= 0) return {};
  std::string result(static_cast<size_t>(size), '\0');
  if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text.data(),
      static_cast<int>(text.size()), result.data(), size, nullptr, nullptr) == 0) {
    return {};
  }
  return result;
}

bool EncodeBitmap(HBITMAP bitmap, ClipboardContents* output) {
  BITMAP info{};
  if (GetObjectW(bitmap, sizeof(info), &info) == 0 ||
      info.bmWidth <= 0 || info.bmHeight <= 0) return false;
  if (static_cast<uint64_t>(info.bmWidth) * info.bmHeight > kMaxPixels) {
    output->error = "clipboard-too-large";
    return false;
  }
  ComPtr<IWICImagingFactory> factory;
  ComPtr<IWICBitmap> source;
  ComPtr<IStream> stream;
  ComPtr<IWICBitmapEncoder> encoder;
  ComPtr<IWICBitmapFrameEncode> frame;
  if (FAILED(CoCreateInstance(CLSID_WICImagingFactory, nullptr,
          CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&factory))) ||
      // CF_BITMAP does not define alpha. PNG clipboard data is preferred below
      // for transparent images; ignoring unused bitmap alpha avoids black shots.
      FAILED(factory->CreateBitmapFromHBITMAP(bitmap, nullptr,
          WICBitmapIgnoreAlpha, &source)) ||
      FAILED(CreateStreamOnHGlobal(nullptr, TRUE, &stream)) ||
      FAILED(factory->CreateEncoder(GUID_ContainerFormatPng, nullptr, &encoder)) ||
      FAILED(encoder->Initialize(stream.Get(), WICBitmapEncoderNoCache)) ||
      FAILED(encoder->CreateNewFrame(&frame, nullptr)) ||
      FAILED(frame->Initialize(nullptr)) ||
      FAILED(frame->SetSize(static_cast<UINT>(info.bmWidth),
                           static_cast<UINT>(info.bmHeight)))) return false;
  WICPixelFormatGUID format = GUID_WICPixelFormat24bppBGR;
  if (FAILED(frame->SetPixelFormat(&format)) ||
      FAILED(frame->WriteSource(source.Get(), nullptr)) ||
      FAILED(frame->Commit()) || FAILED(encoder->Commit())) return false;
  STATSTG stat{};
  if (FAILED(stream->Stat(&stat, STATFLAG_NONAME))) return false;
  if (stat.cbSize.QuadPart > kMaxBytes) {
    output->error = "clipboard-too-large";
    return false;
  }
  LARGE_INTEGER beginning{};
  if (FAILED(stream->Seek(beginning, STREAM_SEEK_SET, nullptr))) return false;
  output->png.resize(static_cast<size_t>(stat.cbSize.QuadPart));
  ULONG read = 0;
  if (FAILED(stream->Read(output->png.data(),
          static_cast<ULONG>(output->png.size()), &read)) ||
      read != output->png.size()) {
    output->png.clear();
    return false;
  }
  return true;
}
}  // namespace

ClipboardContents ReadClipboardAttachments(HWND owner) {
  ClipboardContents output;
  ClipboardLock clipboard(owner);
  if (!clipboard.opened()) {
    output.error = "clipboard-busy";
    return output;
  }
  // Explorer may also advertise a thumbnail bitmap for copied files.
  if (IsClipboardFormatAvailable(CF_HDROP)) {
    const auto drop = static_cast<HDROP>(GetClipboardData(CF_HDROP));
    if (drop == nullptr) { output.error = "clipboard-unavailable"; return output; }
    const UINT count = DragQueryFileW(drop, 0xffffffff, nullptr, 0);
    if (count > kMaxFiles) { output.error = "clipboard-too-many-files"; return output; }
    for (UINT i = 0; i < count; ++i) {
      const UINT length = DragQueryFileW(drop, i, nullptr, 0);
      if (length == 0 || length > 32767) {
        output.error = "clipboard-unavailable";
        return output;
      }
      std::wstring path(static_cast<size_t>(length) + 1, L'\0');
      if (DragQueryFileW(drop, i, path.data(), length + 1) != length) {
        output.error = "clipboard-unavailable";
        return output;
      }
      path.resize(length);
      auto utf8 = ToUtf8(path);
      if (utf8.empty()) { output.error = "clipboard-unavailable"; return output; }
      output.files.push_back(std::move(utf8));
    }
    return output;
  }
  const UINT png = RegisterClipboardFormatW(L"PNG");
  if (png != 0 && IsClipboardFormatAvailable(png)) {
    const auto handle = GetClipboardData(png);
    const SIZE_T size = handle == nullptr ? 0 : GlobalSize(handle);
    if (size > kMaxBytes) { output.error = "clipboard-too-large"; return output; }
    const auto* bytes = handle == nullptr ? nullptr
        : static_cast<const uint8_t*>(GlobalLock(handle));
    constexpr std::array<uint8_t, 8> signature{137, 80, 78, 71, 13, 10, 26, 10};
    if (bytes != nullptr) {
      if (size >= 24 &&
          std::equal(signature.begin(), signature.end(), bytes)) {
        const auto dimension = [bytes](size_t offset) {
          return (static_cast<uint32_t>(bytes[offset]) << 24) |
                 (static_cast<uint32_t>(bytes[offset + 1]) << 16) |
                 (static_cast<uint32_t>(bytes[offset + 2]) << 8) |
                 static_cast<uint32_t>(bytes[offset + 3]);
        };
        if (static_cast<uint64_t>(dimension(16)) * dimension(20) > kMaxPixels) {
          output.error = "clipboard-too-large";
        } else {
          output.png.assign(bytes, bytes + size);
        }
      }
      GlobalUnlock(handle);
    }
    if (!output.png.empty() || !output.error.empty()) return output;
  }
  // Windows synthesizes CF_BITMAP from CF_DIB/CF_DIBV5, including screenshots.
  if (IsClipboardFormatAvailable(CF_BITMAP)) {
    const auto bitmap = static_cast<HBITMAP>(GetClipboardData(CF_BITMAP));
    if (bitmap != nullptr && EncodeBitmap(bitmap, &output)) return output;
    if (output.error.empty()) output.error = "clipboard-unavailable";
  }
  return output;
}

#include "clipboard_reader.h"

#include <shellapi.h>

#include <cstring>

#include "utils.h"

namespace {

// Matches the Web client's maxImagePixels and bounds one encoded payload.
constexpr int64_t kMaxPixels = 40'000'000;
constexpr SIZE_T kMaxPngBytes = 64 * 1024 * 1024;

class ScopedClipboard {
 public:
  explicit ScopedClipboard(HWND owner) {
    for (int attempt = 0; attempt < 5 && !open_; ++attempt) {
      open_ = ::OpenClipboard(owner) != FALSE;
      if (!open_) ::Sleep(10);
    }
  }
  ~ScopedClipboard() {
    if (open_) ::CloseClipboard();
  }
  ScopedClipboard(const ScopedClipboard&) = delete;
  ScopedClipboard& operator=(const ScopedClipboard&) = delete;
  bool open() const { return open_; }

 private:
  bool open_ = false;
};

class ScopedGlobalLock {
 public:
  explicit ScopedGlobalLock(HANDLE handle)
      : handle_(handle), data_(handle ? ::GlobalLock(handle) : nullptr) {}
  ~ScopedGlobalLock() {
    if (data_) ::GlobalUnlock(handle_);
  }
  ScopedGlobalLock(const ScopedGlobalLock&) = delete;
  ScopedGlobalLock& operator=(const ScopedGlobalLock&) = delete;
  const void* data() const { return data_; }
  SIZE_T size() const { return data_ ? ::GlobalSize(handle_) : 0; }

 private:
  HANDLE handle_;
  void* data_;
};

void ReadFiles(ClipboardContent* content) {
  if (!::IsClipboardFormatAvailable(CF_HDROP)) return;
  auto drop = static_cast<HDROP>(::GetClipboardData(CF_HDROP));
  if (drop == nullptr) return;
  const UINT count = ::DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
  for (UINT index = 0; index < count; ++index) {
    const UINT length = ::DragQueryFileW(drop, index, nullptr, 0);
    if (length == 0) continue;
    std::wstring path(static_cast<size_t>(length) + 1, L'\0');
    const UINT copied = ::DragQueryFileW(drop, index, path.data(), length + 1);
    if (copied == 0) continue;
    path.resize(copied);
    std::string utf8 = Utf8FromUtf16(path.c_str());
    if (!utf8.empty()) content->files.push_back(std::move(utf8));
  }
}

void ReadPng(ClipboardContent* content) {
  static const UINT png_format = ::RegisterClipboardFormatW(L"PNG");
  if (png_format == 0 || !::IsClipboardFormatAvailable(png_format)) return;
  ScopedGlobalLock lock(::GetClipboardData(png_format));
  static const uint8_t kSignature[8] = {0x89, 'P',  'N',  'G',
                                        0x0D, 0x0A, 0x1A, 0x0A};
  const auto* bytes = static_cast<const uint8_t*>(lock.data());
  const SIZE_T size = lock.size();
  if (bytes == nullptr || size < sizeof(kSignature) || size > kMaxPngBytes ||
      std::memcmp(bytes, kSignature, sizeof(kSignature)) != 0) {
    return;
  }
  // GlobalSize may round up; Dart trims the stream at its IEND chunk.
  content->png.assign(bytes, bytes + size);
}

void ReadBitmap(ClipboardContent* content) {
  if (!::IsClipboardFormatAvailable(CF_DIB)) return;
  ScopedGlobalLock lock(::GetClipboardData(CF_DIB));
  const auto* info = static_cast<const BITMAPINFO*>(lock.data());
  const SIZE_T size = lock.size();
  if (info == nullptr || size < sizeof(BITMAPINFOHEADER)) return;
  const BITMAPINFOHEADER& header = info->bmiHeader;
  if (header.biSize < sizeof(BITMAPINFOHEADER) || header.biSize > size) return;
  const int64_t width = header.biWidth;
  const int64_t height = header.biHeight < 0 ? -static_cast<int64_t>(header.biHeight)
                                             : header.biHeight;
  if (width <= 0 || height <= 0 || width * height > kMaxPixels) return;
  const WORD bits = header.biBitCount;
  if (bits != 1 && bits != 4 && bits != 8 && bits != 16 && bits != 24 &&
      bits != 32) {
    return;
  }
  if (header.biCompression != BI_RGB && header.biCompression != BI_BITFIELDS) {
    return;
  }
  uint64_t palette = 0;
  if (bits <= 8) {
    palette = header.biClrUsed != 0 ? header.biClrUsed : (1ull << bits);
  }
  // A plain BITMAPINFOHEADER keeps its three channel masks after the header;
  // V4/V5 headers carry them inside.
  const uint64_t masks = header.biCompression == BI_BITFIELDS &&
                                 header.biSize == sizeof(BITMAPINFOHEADER)
                             ? 3
                             : 0;
  const uint64_t offset = header.biSize + (palette + masks) * sizeof(RGBQUAD);
  const uint64_t stride = ((static_cast<uint64_t>(width) * bits + 31) / 32) * 4;
  if (offset + stride * static_cast<uint64_t>(height) > size) return;

  BITMAPINFO target = {};
  target.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  target.bmiHeader.biWidth = static_cast<LONG>(width);
  target.bmiHeader.biHeight = -static_cast<LONG>(height);
  target.bmiHeader.biPlanes = 1;
  target.bmiHeader.biBitCount = 32;
  target.bmiHeader.biCompression = BI_RGB;
  HDC dc = ::CreateCompatibleDC(nullptr);
  if (dc == nullptr) return;
  void* pixels = nullptr;
  HBITMAP bitmap =
      ::CreateDIBSection(dc, &target, DIB_RGB_COLORS, &pixels, nullptr, 0);
  if (bitmap != nullptr && pixels != nullptr) {
    HGDIOBJ previous = ::SelectObject(dc, bitmap);
    const int lines = ::SetDIBitsToDevice(
        dc, 0, 0, static_cast<DWORD>(width), static_cast<DWORD>(height), 0, 0,
        0, static_cast<UINT>(height),
        reinterpret_cast<const uint8_t*>(info) + offset, info, DIB_RGB_COLORS);
    ::SelectObject(dc, previous);
    ::GdiFlush();
    if (lines > 0) {
      const size_t count = static_cast<size_t>(width * height * 4);
      const auto* source = static_cast<const uint8_t*>(pixels);
      content->bgra.assign(source, source + count);
      // CF_DIB leaves the fourth byte undefined; render it opaque.
      for (size_t index = 3; index < count; index += 4) {
        content->bgra[index] = 0xFF;
      }
      content->width = static_cast<int32_t>(width);
      content->height = static_cast<int32_t>(height);
    }
  }
  if (bitmap != nullptr) ::DeleteObject(bitmap);
  ::DeleteDC(dc);
}

}  // namespace

bool ReadClipboardContent(HWND owner, ClipboardContent* content,
                          std::string* error) {
  ScopedClipboard clipboard(owner);
  if (!clipboard.open()) {
    if (error) *error = "The clipboard is in use by another application";
    return false;
  }
  ReadFiles(content);
  if (!content->files.empty()) return true;
  ReadPng(content);
  if (content->png.empty()) ReadBitmap(content);
  return true;
}

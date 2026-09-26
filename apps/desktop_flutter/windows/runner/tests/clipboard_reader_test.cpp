// Exercises real WIC encoding and shell file-list decoding without touching
// the user's clipboard. Only clipboard ownership/access functions are faked.
#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>

#include <cassert>
#include <cstring>
#include <iostream>
#include <map>

namespace {
std::map<UINT, HANDLE> formats;
bool busy = false;
int opened = 0;
BOOL WINAPI TestOpenClipboard(HWND) { if (busy) return FALSE; ++opened; return TRUE; }
BOOL WINAPI TestCloseClipboard() { --opened; return TRUE; }
BOOL WINAPI TestFormatAvailable(UINT format) { return formats.count(format) != 0; }
HANDLE WINAPI TestClipboardData(UINT format) { return formats[format]; }
}
#define OpenClipboard TestOpenClipboard
#define CloseClipboard TestCloseClipboard
#define IsClipboardFormatAvailable TestFormatAvailable
#define GetClipboardData TestClipboardData
#include "../clipboard_reader.cpp"
#undef OpenClipboard
#undef CloseClipboard
#undef IsClipboardFormatAvailable
#undef GetClipboardData

HGLOBAL CopyData(const void* data, size_t size) {
  HGLOBAL handle = GlobalAlloc(GMEM_MOVEABLE, size);
  assert(handle != nullptr);
  void* bytes = GlobalLock(handle);
  assert(bytes != nullptr);
  std::memcpy(bytes, data, size);
  GlobalUnlock(handle);
  return handle;
}

int main() {
  assert(SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)));
  const UINT png_format = RegisterClipboardFormatW(L"PNG");
  auto empty = ReadClipboardAttachments(nullptr);
  assert(empty.error.empty() && empty.png.empty() && empty.files.empty());
  busy = true;
  assert(ReadClipboardAttachments(nullptr).error == "clipboard-busy");
  busy = false;

  BITMAPINFO info{};
  info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
  info.bmiHeader.biWidth = 2;
  info.bmiHeader.biHeight = -2;
  info.bmiHeader.biPlanes = 1;
  info.bmiHeader.biBitCount = 32;
  info.bmiHeader.biCompression = BI_RGB;
  void* pixels = nullptr;
  HBITMAP bitmap = CreateDIBSection(nullptr, &info, DIB_RGB_COLORS, &pixels, nullptr, 0);
  assert(bitmap != nullptr && pixels != nullptr);
  // Screenshot alpha is commonly all zero and must not become transparent.
  const uint32_t red[4] = {0x00ff0000, 0x00ff0000, 0x00ff0000, 0x00ff0000};
  std::memcpy(pixels, red, sizeof(red));
  formats[CF_BITMAP] = bitmap;
  const auto screenshot = ReadClipboardAttachments(nullptr);
  assert(screenshot.error.empty() && screenshot.png.size() > 24);
  assert(screenshot.png[0] == 137 && screenshot.png[1] == 80);
  assert(screenshot.png[19] == 2 && screenshot.png[23] == 2);

  // Decode the encoded image to verify actual color and opaque bitmap alpha.
  Microsoft::WRL::ComPtr<IWICImagingFactory> factory;
  Microsoft::WRL::ComPtr<IWICStream> stream;
  Microsoft::WRL::ComPtr<IWICBitmapDecoder> decoder;
  Microsoft::WRL::ComPtr<IWICBitmapFrameDecode> frame;
  Microsoft::WRL::ComPtr<IWICFormatConverter> rgba;
  assert(SUCCEEDED(CoCreateInstance(CLSID_WICImagingFactory, nullptr,
      CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&factory))));
  assert(SUCCEEDED(factory->CreateStream(&stream)));
  auto encoded = screenshot.png;
  assert(SUCCEEDED(stream->InitializeFromMemory(encoded.data(), static_cast<DWORD>(encoded.size()))));
  assert(SUCCEEDED(factory->CreateDecoderFromStream(stream.Get(), nullptr, WICDecodeMetadataCacheOnLoad, &decoder)));
  assert(SUCCEEDED(decoder->GetFrame(0, &frame)));
  assert(SUCCEEDED(factory->CreateFormatConverter(&rgba)));
  assert(SUCCEEDED(rgba->Initialize(frame.Get(), GUID_WICPixelFormat32bppRGBA,
      WICBitmapDitherTypeNone, nullptr, 0, WICBitmapPaletteTypeCustom)));
  uint8_t decoded[16]{};
  assert(SUCCEEDED(rgba->CopyPixels(nullptr, 8, sizeof(decoded), decoded)));
  assert(decoded[0] == 255 && decoded[1] == 0 && decoded[2] == 0 && decoded[3] == 255);

  HGLOBAL png = CopyData(screenshot.png.data(), screenshot.png.size());
  formats[png_format] = png;
  assert(ReadClipboardAttachments(nullptr).png == screenshot.png);

  const wchar_t paths[] = L"C:\\images\\photo.png\0C:\\docs\\report.pdf\0";
  std::vector<uint8_t> drop_bytes(sizeof(DROPFILES) + sizeof(paths));
  auto* drop_info = reinterpret_cast<DROPFILES*>(drop_bytes.data());
  drop_info->pFiles = sizeof(DROPFILES);
  drop_info->fWide = TRUE;
  std::memcpy(drop_bytes.data() + sizeof(DROPFILES), paths, sizeof(paths));
  HGLOBAL drop = CopyData(drop_bytes.data(), drop_bytes.size());
  formats[CF_HDROP] = drop;
  const auto files = ReadClipboardAttachments(nullptr);
  assert(files.error.empty() && files.png.empty() && files.files.size() == 2);
  assert(files.files[0] == "C:\\images\\photo.png");
  formats.erase(CF_HDROP);

  std::wstring many_paths;
  for (int i = 0; i < 9; ++i) {
    many_paths += L"C:\\images\\photo.png";
    many_paths.push_back(L'\0');
  }
  many_paths.push_back(L'\0');
  std::vector<uint8_t> many_drop_bytes(sizeof(DROPFILES) + many_paths.size() * sizeof(wchar_t));
  auto* many_drop_info = reinterpret_cast<DROPFILES*>(many_drop_bytes.data());
  many_drop_info->pFiles = sizeof(DROPFILES);
  many_drop_info->fWide = TRUE;
  std::memcpy(many_drop_bytes.data() + sizeof(DROPFILES), many_paths.data(),
      many_paths.size() * sizeof(wchar_t));
  HGLOBAL many_drop = CopyData(many_drop_bytes.data(), many_drop_bytes.size());
  formats[CF_HDROP] = many_drop;
  assert(ReadClipboardAttachments(nullptr).error == "clipboard-too-many-files");
  formats.erase(CF_HDROP);

  auto excessive_dimensions = screenshot.png;
  // Claim a 32768 x 32768 PNG: reject before decoding or allocating pixels.
  for (size_t offset : {size_t{16}, size_t{20}}) {
    excessive_dimensions[offset] = 0;
    excessive_dimensions[offset + 1] = 0;
    excessive_dimensions[offset + 2] = 128;
    excessive_dimensions[offset + 3] = 0;
  }
  HGLOBAL huge_png = CopyData(excessive_dimensions.data(), excessive_dimensions.size());
  formats[png_format] = huge_png;
  assert(ReadClipboardAttachments(nullptr).error == "clipboard-too-large");

  HGLOBAL large = GlobalAlloc(GMEM_MOVEABLE, 16 * 1024 * 1024 + 1);
  assert(large != nullptr);
  formats[png_format] = large;
  assert(ReadClipboardAttachments(nullptr).error == "clipboard-too-large");
  assert(opened == 0);
  formats.clear();
  GlobalFree(large);
  GlobalFree(drop);
  GlobalFree(many_drop);
  GlobalFree(huge_png);
  GlobalFree(png);
  DeleteObject(bitmap);
  rgba.Reset(); frame.Reset(); decoder.Reset(); stream.Reset(); factory.Reset();
  CoUninitialize();
  std::cout << "PASS: clipboard busy/empty, screenshot PNG pixels/alpha, PNG preference, file precedence, count/size/pixel bounds, lock cleanup\n";
  return 0;
}

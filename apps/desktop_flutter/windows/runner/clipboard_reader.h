#ifndef RUNNER_CLIPBOARD_READER_H_
#define RUNNER_CLIPBOARD_READER_H_

#include <windows.h>

#include <cstdint>
#include <string>
#include <vector>

// Clipboard content that Flutter's text-only clipboard API cannot expose.
// Copied files take precedence; image content is read only without files.
// `png` carries the encoded bytes the source application published, otherwise
// `bgra` carries opaque top-down pixels rendered from CF_DIB.
struct ClipboardContent {
  std::vector<std::string> files;
  std::vector<uint8_t> png;
  std::vector<uint8_t> bgra;
  int32_t width = 0;
  int32_t height = 0;
};

// Reads the current clipboard. Returns false with `error` only when another
// process keeps the clipboard open through the bounded retries.
bool ReadClipboardContent(HWND owner, ClipboardContent* content,
                          std::string* error);

#endif  // RUNNER_CLIPBOARD_READER_H_

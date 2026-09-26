#ifndef RUNNER_CLIPBOARD_READER_H_
#define RUNNER_CLIPBOARD_READER_H_

#include <windows.h>

#include <cstdint>
#include <string>
#include <vector>

struct ClipboardContents {
  std::vector<uint8_t> png;
  std::vector<std::string> files;
  std::string error;
};

// Owns only copies of clipboard data; never frees handles owned by Windows.
ClipboardContents ReadClipboardAttachments(HWND owner);

#endif  // RUNNER_CLIPBOARD_READER_H_

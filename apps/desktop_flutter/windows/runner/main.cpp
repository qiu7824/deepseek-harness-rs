#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <flutter_windows.h>
#include <windows.h>
#include <algorithm>
#include <memory>

#include "flutter_window.h"
#include "utils.h"

namespace {

// Fits the default window inside the primary work area, centered, so the
// bottom of the sidebar never opens underneath the taskbar.
void FitToWorkArea(Win32Window::Point* origin, Win32Window::Size* size) {
  RECT work{};
  if (!::SystemParametersInfoW(SPI_GETWORKAREA, 0, &work, 0)) return;
  const POINT corner{work.left, work.top};
  const UINT dpi = FlutterDesktopGetDpiForMonitor(
      ::MonitorFromPoint(corner, MONITOR_DEFAULTTOPRIMARY));
  const double scale = dpi / 96.0;
  const int work_width = static_cast<int>((work.right - work.left) / scale);
  const int work_height = static_cast<int>((work.bottom - work.top) / scale);
  constexpr int kMargin = 24;
  const int width = std::max(
      720, std::min(static_cast<int>(size->width), work_width - kMargin * 2));
  const int height = std::max(
      520, std::min(static_cast<int>(size->height), work_height - kMargin * 2));
  origin->x = static_cast<unsigned int>(
      static_cast<int>(work.left / scale) + std::max(0, (work_width - width) / 2));
  origin->y = static_cast<unsigned int>(
      static_cast<int>(work.top / scale) + std::max(0, (work_height - height) / 2));
  size->width = static_cast<unsigned int>(width);
  size->height = static_cast<unsigned int>(height);
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  HANDLE singleton = ::CreateMutexW(
      nullptr, FALSE, L"Local\\DeepSeekHarnessFlutterDesktop");
  const DWORD singleton_error = ::GetLastError();
  if (singleton == nullptr) return EXIT_FAILURE;
  const auto close_mutex = [](void* handle) { ::CloseHandle(handle); };
  std::unique_ptr<void, decltype(close_mutex)> instance_lock(singleton, close_mutex);
  HWND existing = ::FindWindowW(L"FLUTTER_RUNNER_WIN32_WINDOW", L"DeepSeek Harness");
  if (singleton_error == ERROR_ALREADY_EXISTS || existing != nullptr) {
    for (int attempt = 0; existing == nullptr && attempt < 40; ++attempt) {
      ::Sleep(50);
      existing = ::FindWindowW(L"FLUTTER_RUNNER_WIN32_WINDOW", L"DeepSeek Harness");
    }
    if (existing != nullptr) {
      ::ShowWindow(existing, ::IsIconic(existing) ? SW_RESTORE : SW_SHOW);
      ::SetForegroundWindow(existing);
    }
    return EXIT_SUCCESS;
  }
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  int exit_code = EXIT_SUCCESS;
  {
    flutter::DartProject project(L"data");
    project.set_impeller_switch(flutter::ImpellerSwitch::Disabled);

    std::vector<std::string> command_line_arguments =
        GetCommandLineArguments();

    project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

    FlutterWindow window(project);
    Win32Window::Point origin(10, 10);
    Win32Window::Size size(1440, 900);
    FitToWorkArea(&origin, &size);
    if (window.Create(L"DeepSeek Harness", origin, size)) {
      window.SetQuitOnClose(true);

      ::MSG msg;
      while (::GetMessage(&msg, nullptr, 0, 0)) {
        ::TranslateMessage(&msg);
        ::DispatchMessage(&msg);
      }
    } else {
      exit_code = EXIT_FAILURE;
    }
    // The window_manager plugin ends the loop with a quit message while the
    // window still exists. The window, engine and COM-backed plugins (drag
    // and drop, speech, voice input) are released here, before COM itself;
    // releasing them afterwards crashed flutter_windows.dll on every close.
  }

  ::CoUninitialize();
  return exit_code;
}

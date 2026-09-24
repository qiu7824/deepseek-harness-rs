#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <memory>

#include "flutter_window.h"
#include "utils.h"

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

  flutter::DartProject project(L"data");
  project.set_impeller_switch(flutter::ImpellerSwitch::Disabled);

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1440, 900);
  if (!window.Create(L"DeepSeek Harness", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}

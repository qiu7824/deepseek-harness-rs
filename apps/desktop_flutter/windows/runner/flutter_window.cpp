#include "flutter_window.h"

#include <optional>

#include "flutter/generated_plugin_registrant.h"
#include "clipboard_reader.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  clipboard_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "dsh/clipboard",
          &flutter::StandardMethodCodec::GetInstance());
  clipboard_channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    if (call.method_name() != "read") { result->NotImplemented(); return; }
    auto contents = ReadClipboardAttachments(GetHandle());
    if (!contents.error.empty()) {
      result->Error(contents.error, "Unable to read clipboard attachments");
      return;
    }
    flutter::EncodableList files;
    for (auto& path : contents.files) files.emplace_back(std::move(path));
    result->Success(flutter::EncodableValue(flutter::EncodableMap{
        {flutter::EncodableValue("png"), flutter::EncodableValue(std::move(contents.png))},
        {flutter::EncodableValue("files"), flutter::EncodableValue(std::move(files))}}));
  });
  theme_channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "dsh/window-theme",
      &flutter::StandardMethodCodec::GetInstance());
  theme_channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    if (call.method_name() != "setDarkMode") { result->NotImplemented(); return; }
    const auto* dark = call.arguments() ? std::get_if<bool>(call.arguments()) : nullptr;
    if (!dark) { result->Error("invalid-theme", "Expected a boolean"); return; }
    SetAppDarkMode(*dark);
    result->Success();
  });
  voice_channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "dsh/voice",
      &flutter::StandardMethodCodec::GetInstance());
  voice_channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) {
        if (call.method_name() == "start") {
          const auto* generation = call.arguments() == nullptr ? nullptr
              : std::get_if<std::string>(call.arguments());
          if (generation == nullptr || generation->empty()) {
            result->Error("invalid-generation", "Missing voice generation");
          } else if (!voice_recognizer_.Start(*generation)) {
            result->Error("voice-busy", "Voice recognizer is stopping; retry shortly");
          } else { result->Success(); }
        } else if (call.method_name() == "stop") {
          voice_recognizer_.Stop();
          result->Success();
        } else if (call.method_name() == "poll") {
          flutter::EncodableList events;
          for (const auto& event : voice_recognizer_.Drain()) {
            events.emplace_back(flutter::EncodableMap{
              {flutter::EncodableValue("kind"), flutter::EncodableValue(event.kind)},
              {flutter::EncodableValue("text"), flutter::EncodableValue(event.text)},
              {flutter::EncodableValue("generation"), flutter::EncodableValue(event.generation)}});
          }
          // Only the platform thread replies to Dart; the worker owns no channel.
          result->Success(flutter::EncodableValue(events));
        } else {
          result->NotImplemented();
        }
      });
  speech_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "dsh/read-aloud",
          &flutter::StandardMethodCodec::GetInstance());
  speech_channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) {
        const auto* args = call.arguments() == nullptr
                               ? nullptr
                               : std::get_if<flutter::EncodableMap>(call.arguments());
        if (args == nullptr) {
          result->Error("invalid-speech", "Expected a speech argument map");
          return;
        }
        const auto generation_it = args->find(flutter::EncodableValue("generation"));
        const auto* generation = generation_it == args->end()
                                     ? nullptr
                                     : std::get_if<std::string>(&generation_it->second);
        if (generation == nullptr || generation->empty()) {
          result->Error("invalid-speech", "Missing speech generation");
          return;
        }
        if (call.method_name() == "start") {
          const auto text_it = args->find(flutter::EncodableValue("text"));
          const auto* text = text_it == args->end()
                                 ? nullptr
                                 : std::get_if<std::string>(&text_it->second);
          std::string error;
          if (text == nullptr ||
              !speech_speaker_.Start(*text, *generation, &error)) {
            result->Error("speech-unavailable", error.empty() ? "Invalid speech text" : error);
          } else {
            result->Success();
          }
        } else if (call.method_name() == "status") {
          bool done = false;
          std::string error;
          if (!speech_speaker_.Done(*generation, &done, &error)) {
            result->Error("speech-unavailable", error);
          } else {
            result->Success(flutter::EncodableValue(done));
          }
        } else if (call.method_name() == "stop") {
          speech_speaker_.Stop(*generation);
          result->Success();
        } else {
          result->NotImplemented();
        }
      });
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  if (clipboard_channel_) clipboard_channel_->SetMethodCallHandler(nullptr);
  clipboard_channel_.reset();
  if (theme_channel_) theme_channel_->SetMethodCallHandler(nullptr);
  theme_channel_.reset();
  voice_recognizer_.Shutdown();
  if (voice_channel_) voice_channel_->SetMethodCallHandler(nullptr);
  voice_channel_.reset();
  speech_speaker_.Shutdown();
  if (speech_channel_) speech_channel_->SetMethodCallHandler(nullptr);
  speech_channel_.reset();
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

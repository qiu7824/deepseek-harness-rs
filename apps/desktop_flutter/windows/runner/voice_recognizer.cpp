#include "voice_recognizer.h"

#include <windows.h>
#include <sapi.h>
#include <wrl/client.h>
#include <chrono>
#include <cstdio>

namespace {
using Microsoft::WRL::ComPtr;
std::string Utf8(const wchar_t* text) {
  const int size = WideCharToMultiByte(CP_UTF8, 0, text, -1, nullptr, 0, nullptr, nullptr);
  if (size <= 1) return {};
  std::string value(static_cast<size_t>(size), '\0');
  WideCharToMultiByte(CP_UTF8, 0, text, -1, value.data(), size, nullptr, nullptr);
  value.resize(static_cast<size_t>(size - 1));
  return value;
}
HRESULT DefaultAudio(ISpObjectToken** output) {
  ComPtr<ISpObjectTokenCategory> category;
  HRESULT hr = CoCreateInstance(CLSID_SpObjectTokenCategory, nullptr,
      CLSCTX_INPROC_SERVER, IID_PPV_ARGS(category.GetAddressOf()));
  if (SUCCEEDED(hr)) hr = category->SetId(SPCAT_AUDIOIN, FALSE);
  wchar_t* id = nullptr;
  if (SUCCEEDED(hr)) hr = category->GetDefaultTokenId(&id);
  ComPtr<ISpObjectToken> token;
  if (SUCCEEDED(hr)) hr = CoCreateInstance(CLSID_SpObjectToken, nullptr,
      CLSCTX_INPROC_SERVER, IID_PPV_ARGS(token.GetAddressOf()));
  if (SUCCEEDED(hr)) hr = token->SetId(nullptr, id, FALSE);
  CoTaskMemFree(id);
  if (SUCCEEDED(hr)) *output = token.Detach();
  return hr;
}
HRESULT SelectUserLanguage(ISpRecognizer* recognizer) {
  ComPtr<ISpObjectTokenCategory> category;
  HRESULT hr = CoCreateInstance(CLSID_SpObjectTokenCategory, nullptr,
      CLSCTX_INPROC_SERVER, IID_PPV_ARGS(category.GetAddressOf()));
  if (SUCCEEDED(hr)) hr = category->SetId(SPCAT_RECOGNIZERS, FALSE);
  wchar_t attributes[32];
  swprintf_s(attributes, L"Language=%x", static_cast<unsigned>(GetUserDefaultUILanguage()));
  ComPtr<IEnumSpObjectTokens> tokens;
  if (SUCCEEDED(hr)) hr = category->EnumTokens(attributes, nullptr, tokens.GetAddressOf());
  ComPtr<ISpObjectToken> token;
  ULONG fetched = 0;
  if (SUCCEEDED(hr)) hr = tokens->Next(1, token.GetAddressOf(), &fetched);
  // Older Windows installations may have only their default recognizer.
  if (FAILED(hr) || fetched == 0) return S_OK;
  return recognizer->SetRecognizer(token.Get());
}
}

VoiceRecognizer::~VoiceRecognizer() { Shutdown(); }

bool VoiceRecognizer::Start(const std::string& generation) {
  if (running_) return false;
  // Failed workers must also be joined before their thread object is reused.
  if (worker_.joinable()) worker_.join();
  { std::lock_guard<std::mutex> lock(mutex_); events_.clear(); }
  stop_ = false;
  running_ = true;
  worker_ = std::thread([this, generation] { Run(generation); });
  return true;
}
void VoiceRecognizer::Stop() { stop_ = true; }
void VoiceRecognizer::Shutdown() {
  Stop();
  if (worker_.joinable()) worker_.join();
  std::lock_guard<std::mutex> lock(mutex_);
  events_.clear();
}
std::vector<VoiceRecognizer::Event> VoiceRecognizer::Drain() {
  std::lock_guard<std::mutex> lock(mutex_);
  std::vector<Event> result;
  result.swap(events_);
  return result;
}
void VoiceRecognizer::Publish(Event event) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (events_.size() >= 64 || event.text.size() > 65536) {
    events_.clear();
    events_.push_back({"error", "buffer-limit", event.generation});
    stop_ = true;
    return;
  }
  events_.push_back(std::move(event));
}
void VoiceRecognizer::Run(const std::string& generation) {
  HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  const bool initialized = SUCCEEDED(hr);
  const char* stage = "initialize";
  {
    ComPtr<ISpRecognizer> recognizer;
    ComPtr<ISpRecoContext> context;
    ComPtr<ISpRecoGrammar> grammar;
    ComPtr<ISpObjectToken> audio;
    if (SUCCEEDED(hr) && !stop_) {
      stage = "recognizer";
      hr = CoCreateInstance(CLSID_SpInprocRecognizer, nullptr, CLSCTX_INPROC_SERVER,
          IID_PPV_ARGS(recognizer.GetAddressOf()));
    }
    if (SUCCEEDED(hr) && !stop_) { stage = "language"; hr = SelectUserLanguage(recognizer.Get()); }
    if (SUCCEEDED(hr) && !stop_) { stage = "microphone"; hr = DefaultAudio(audio.GetAddressOf()); }
    if (SUCCEEDED(hr) && !stop_) hr = recognizer->SetInput(audio.Get(), TRUE);
    if (SUCCEEDED(hr) && !stop_) { stage = "context"; hr = recognizer->CreateRecoContext(context.GetAddressOf()); }
    const ULONGLONG interests = SPFEI(SPEI_RECOGNITION) | SPFEI(SPEI_HYPOTHESIS);
    if (SUCCEEDED(hr) && !stop_) hr = context->SetInterest(interests, interests);
    if (SUCCEEDED(hr) && !stop_) { stage = "dictation"; hr = context->CreateGrammar(1, grammar.GetAddressOf()); }
    if (SUCCEEDED(hr) && !stop_) hr = grammar->LoadDictation(nullptr, SPLO_STATIC);
    if (SUCCEEDED(hr) && !stop_) hr = grammar->SetDictationState(SPRS_ACTIVE);
    if (SUCCEEDED(hr) && !stop_) { stage = "activate"; hr = recognizer->SetRecoState(SPRST_ACTIVE); }
    if (SUCCEEDED(hr) && !stop_) Publish({"ready", "", generation});
    while (SUCCEEDED(hr) && !stop_) {
      SPEVENT event{};
      ULONG count = 0;
      hr = context->GetEvents(1, &event, &count);
      if (count == 1) {
        if ((event.eEventId == SPEI_RECOGNITION || event.eEventId == SPEI_HYPOTHESIS) && event.lParam != 0) {
          auto* result = reinterpret_cast<ISpRecoResult*>(event.lParam);
          wchar_t* text = nullptr;
          if (SUCCEEDED(result->GetText(static_cast<ULONG>(SP_GETWHOLEPHRASE),
              static_cast<ULONG>(SP_GETWHOLEPHRASE), TRUE, &text, nullptr))) {
            if (text != nullptr && !stop_) Publish({
                event.eEventId == SPEI_HYPOTHESIS ? "partial" : "result", Utf8(text), generation});
            CoTaskMemFree(text);
          }
        }
        if ((event.elParamType == SPET_LPARAM_IS_OBJECT || event.elParamType == SPET_LPARAM_IS_TOKEN) && event.lParam)
          reinterpret_cast<IUnknown*>(event.lParam)->Release();
        else if ((event.elParamType == SPET_LPARAM_IS_POINTER || event.elParamType == SPET_LPARAM_IS_STRING) && event.lParam)
          CoTaskMemFree(reinterpret_cast<void*>(event.lParam));
      } else {
        std::this_thread::sleep_for(std::chrono::milliseconds(24));
      }
    }
    if (FAILED(hr) && !stop_) {
      char code[96];
      std::snprintf(code, sizeof(code), "%s:0x%08lX", stage, static_cast<unsigned long>(hr));
      Publish({"error", code, generation});
    }
    if (grammar) grammar->SetDictationState(SPRS_INACTIVE);
    if (recognizer) recognizer->SetRecoState(SPRST_INACTIVE);
  }
  if (initialized) CoUninitialize();
  Publish({"stopped", "", generation});
  running_ = false;
}

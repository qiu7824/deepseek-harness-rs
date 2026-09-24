#include "speech_speaker.h"

#include <windows.h>
#include <utility>

bool SpeechSpeaker::Start(const std::string& utf8,
                          const std::string& generation, std::string* error) {
  if (generation.empty() || utf8.empty() || utf8.size() > 32768 ||
      utf8.find('\0') != std::string::npos) {
    *error = "Invalid speech text or generation";
    return false;
  }
  const int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                        utf8.data(), static_cast<int>(utf8.size()),
                                        nullptr, 0);
  if (count <= 0) {
    *error = "Speech text is not valid UTF-8";
    return false;
  }
  std::wstring decoded(static_cast<size_t>(count), L'\0');
  if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(),
                          static_cast<int>(utf8.size()), decoded.data(),
                          count) != count) {
    *error = "Speech text conversion failed";
    return false;
  }
  if (!voice_) {
    const HRESULT created = CoCreateInstance(
        CLSID_SpVoice, nullptr, CLSCTX_ALL,
        IID_PPV_ARGS(voice_.GetAddressOf()));
    if (FAILED(created)) {
      *error = "Windows speech voice is unavailable";
      return false;
    }
  }
  text_ = std::move(decoded);
  generation_ = generation;
  const HRESULT spoken = voice_->Speak(
      text_.c_str(), SPF_ASYNC | SPF_PURGEBEFORESPEAK, nullptr);
  if (FAILED(spoken)) {
    Stop(generation);
    *error = "Windows speech voice could not start";
    return false;
  }
  return true;
}

bool SpeechSpeaker::Done(const std::string& generation, bool* done,
                          std::string* error) {
  if (generation != generation_ || !voice_) {
    *done = true;
    return true;
  }
  const HRESULT status = voice_->WaitUntilDone(0);
  if (FAILED(status)) {
    *error = "Windows speech status is unavailable";
    return false;
  }
  *done = status == S_OK;
  return true;
}

void SpeechSpeaker::Stop(const std::string& generation) {
  if (!generation.empty() && generation != generation_) return;
  if (voice_) {
    voice_->Speak(L"", SPF_ASYNC | SPF_PURGEBEFORESPEAK, nullptr);
    voice_.Reset();
  }
  text_.clear();
  generation_.clear();
}

void SpeechSpeaker::Shutdown() { Stop(""); }

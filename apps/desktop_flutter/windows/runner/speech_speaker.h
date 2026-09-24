#ifndef RUNNER_SPEECH_SPEAKER_H_
#define RUNNER_SPEECH_SPEAKER_H_

#include <sapi.h>
#include <wrl/client.h>

#include <string>

// One SAPI voice, owned by the runner thread that initialized COM.
class SpeechSpeaker {
 public:
  bool Start(const std::string& utf8, const std::string& generation,
             std::string* error);
  bool Done(const std::string& generation, bool* done, std::string* error);
  void Stop(const std::string& generation);
  void Shutdown();

 private:
  Microsoft::WRL::ComPtr<ISpVoice> voice_;
  std::wstring text_;
  std::string generation_;
};

#endif  // RUNNER_SPEECH_SPEAKER_H_

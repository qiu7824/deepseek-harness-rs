#ifndef RUNNER_VOICE_RECOGNIZER_H_
#define RUNNER_VOICE_RECOGNIZER_H_

#include <atomic>
#include <mutex>
#include <string>
#include <vector>
#include <thread>

class VoiceRecognizer {
 public:
  struct Event { std::string kind, text, generation; };

  VoiceRecognizer() = default;
  ~VoiceRecognizer();

  VoiceRecognizer(const VoiceRecognizer&) = delete;
  VoiceRecognizer& operator=(const VoiceRecognizer&) = delete;

  bool Start(const std::string& generation);
  void Stop();
  void Shutdown();
  std::vector<Event> Drain();

 private:
  void Run(const std::string& generation);
  void Publish(Event event);

  std::atomic<bool> running_{false};
  std::atomic<bool> stop_{false};
  std::thread worker_;
  std::mutex mutex_;
  std::vector<Event> events_;
};

#endif  // RUNNER_VOICE_RECOGNIZER_H_

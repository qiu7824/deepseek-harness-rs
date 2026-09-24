// Deterministic SAPI activation failure; never opens a microphone.
#include <windows.h>
#include <sapi.h>
#include <wrl/client.h>
#include <cassert>
#include <chrono>
#include <iostream>
#include <thread>

HRESULT FailActivation(REFCLSID, LPUNKNOWN, DWORD, REFIID, LPVOID*) {
  return E_FAIL;
}
#define CoCreateInstance FailActivation
#include "../voice_recognizer.cpp"
#undef CoCreateInstance

int main() {
  VoiceRecognizer voice;
  for (int cycle = 0; cycle != 32; ++cycle) {
    const std::string generation = std::to_string(cycle);
    assert(voice.Start(generation));
    bool failed = false, stopped = false;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!stopped && std::chrono::steady_clock::now() < deadline) {
      for (const auto& event : voice.Drain()) {
        assert(event.generation == generation);
        assert(event.kind != "ready");
        if (event.kind == "error") failed = true;
        if (event.kind == "stopped") stopped = true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    assert(failed && stopped);
    // Stop must not skip joining a failed worker. Restart used to terminate.
    if (cycle % 2 == 0) voice.Shutdown();
  }
  assert(voice.Start("dispose-during-start"));
  voice.Shutdown();
  std::cout << "PASS: 32 activation failures/restarts and shutdown during start\n";
  return 0;
}

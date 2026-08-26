window.__ModuleLoader__.load({
  id: "dsh-voice-input",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const inject = ["slots"];

    function VoiceInputButton(props) {
      const recognitionRef = React.useRef(null);
      const draftRef = React.useRef(props.input?.draft ?? "");
      draftRef.current = props.input?.draft ?? "";
      const [listening, setListening] = React.useState(false);
      const SpeechRecognition = typeof window === "undefined"
        ? undefined
        : window.SpeechRecognition ?? window.webkitSpeechRecognition;
      const disabled = SpeechRecognition === undefined || props.inputActions === undefined || props.locked || props.input?.phase === "adjudicating" || props.input?.phase === "submitting";

      React.useEffect(() => () => {
        const recognition = recognitionRef.current;
        if (recognition === null) return;
        recognition.onresult = null;
        recognition.onerror = null;
        recognition.onend = null;
        recognition.abort();
        recognitionRef.current = null;
      }, [props.sessionId]);

      if (SpeechRecognition === undefined) return null;
      const toggle = () => {
        if (disabled) return;
        if (recognitionRef.current !== null) {
          recognitionRef.current.stop();
          return;
        }
        let recognition;
        try {
          recognition = new SpeechRecognition();
        } catch (error) {
          setListening(false);
          console.error("dsh-voice-input: recognition unavailable", error);
          return;
        }
        recognition.lang = navigator.language || "zh-CN";
        recognition.interimResults = true;
        recognition.continuous = false;
        let finalText = "";
        recognition.onresult = (event) => {
          for (let index = event.resultIndex; index < event.results.length; index += 1) {
            if (event.results[index].isFinal) finalText += event.results[index][0].transcript;
          }
        };
        recognition.onerror = (event) => console.error("dsh-voice-input: recognition failed", event.error);
        recognition.onend = () => {
          recognitionRef.current = null;
          setListening(false);
          const spoken = finalText.trim();
          if (spoken !== "") {
            const current = draftRef.current;
            props.inputActions.setDraft(`${current}${current === "" || current.endsWith(" ") ? "" : " "}${spoken}`);
          }
        };
        try {
          recognitionRef.current = recognition;
          setListening(true);
          recognition.start();
        } catch (error) {
          recognitionRef.current = null;
          setListening(false);
          console.error("dsh-voice-input: recognition start failed", error);
        }
      };
      return React.createElement("button", {
        type: "button",
        className: "dsh-voice-input-button",
        title: listening ? "停止语音输入" : "语音输入",
        "aria-label": listening ? "停止语音输入" : "语音输入",
        "aria-pressed": listening ? "true" : "false",
        disabled,
        onMouseDown: (event) => event.preventDefault(),
        onClick: toggle,
        style: { width: "28px", height: "28px", display: "inline-flex", alignItems: "center", justifyContent: "center", border: 0, borderRadius: "8px", background: listening ? "var(--dsw-alias-interactive-bg-active,rgba(127,127,137,.16))" : "var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.10))", color: listening ? "var(--dsw-alias-state-error-primary,#d92d20)" : "var(--dsw-alias-label-secondary,#667085)", cursor: disabled ? "not-allowed" : "pointer", opacity: disabled ? .45 : 1 }
      }, React.createElement("svg", { width: 16, height: 16, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: listening ? 2.4 : 1.8, "aria-hidden": true }, React.createElement("rect", { x: 8, y: 3, width: 8, height: 12, rx: 4 }), React.createElement("path", { d: "M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6" })));
    }

    function apply(ctx) {
      ctx.slots.inject("conversation.input.right", () => ctx.slots.register({ name: "conversation.input.right", id: "voice-input", order: 100, label: "语音输入" }, (props) => React.createElement(VoiceInputButton, props)));
    }
    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});

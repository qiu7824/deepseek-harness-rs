async function inspectVideo(selector, time) {
    const video = document.querySelector(selector);
    if (!(video instanceof HTMLVideoElement)) throw Error("No video element matches this selector; open a playable video page first.");
    // Survives separate Runtime.evaluate calls without retaining replaced media.
    // A source URL alone does not identify an element or a srcObject stream.
    const identityKey = Symbol.for("deepseek-harness.video-elements");
    let identities = globalThis[identityKey];
    if (!(identities instanceof WeakMap)) {
        identities = new WeakMap();
        Object.defineProperty(globalThis, identityKey, { value: identities, configurable: true });
    }
    let elementId = identities.get(video);
    if (!elementId) {
        elementId = [...crypto.getRandomValues(new Uint32Array(4))].map(value => value.toString(16).padStart(8, "0")).join("");
        identities.set(video, elementId);
    }
    // MediaSource can report HAVE_METADATA with duration 0 while its timeline is
    // still loading. Do not turn that transient state into a caller timestamp error.
    const timelineReady = () => video.readyState >= 1 && video.duration > 0;
    if (!timelineReady()) await new Promise((resolve, reject) => {
        const events = ["loadedmetadata", "durationchange", "loadeddata"];
        const finish = error => {
            clearTimeout(timer); clearInterval(poll);
            for (const event of events) video.removeEventListener(event, ready);
            video.removeEventListener("error", failed);
            error ? reject(error) : resolve();
        };
        const ready = () => {
            if (document.querySelector(selector) !== video) return finish(Error("Video source changed while loading metadata"));
            if (timelineReady()) finish();
        };
        const failed = () => finish(Error("Video metadata could not be loaded"));
        const timer = setTimeout(() => finish(Error("Video timeline is not ready; wait for playable media")), 5000);
        const poll = setInterval(ready, 50);
        for (const event of events) video.addEventListener(event, ready);
        video.addEventListener("error", failed);
        ready();
    });
    const source = video.currentSrc;
    if (time !== null) {
        if (!Number.isFinite(time) || time < 0 || (Number.isFinite(video.duration) && time >= video.duration)) throw Error("Frame time is outside the video duration");
        video.pause();
        if (Math.abs(video.currentTime - time) > .02 || video.readyState < 2) await new Promise((resolve, reject) => {
            const finish = error => { clearTimeout(timer); video.removeEventListener("seeked", ready); video.removeEventListener("loadeddata", ready); video.removeEventListener("error", failed); error ? reject(error) : resolve(); };
            const ready = () => { if (!video.seeking && video.readyState >= 2) finish(); }, failed = () => finish(Error("Video seeking failed"));
            const timer = setTimeout(() => finish(Error("Video frame timed out; the source may not be seekable")), 5000);
            video.addEventListener("seeked", ready); video.addEventListener("loadeddata", ready); video.addEventListener("error", failed); video.currentTime = time;
        });
        if (!video.videoWidth || !video.videoHeight || video.readyState < 2) throw Error("No decoded video frame is available");
        video.scrollIntoView({ block: "center", inline: "center" });
        await new Promise((resolve, reject) => {
            let first = 0, second = 0;
            const finish = error => {
                clearTimeout(timer); cancelAnimationFrame(first); cancelAnimationFrame(second);
                error ? reject(error) : resolve();
            };
            const timer = setTimeout(() => finish(Error("Video frame layout timed out; make the video tab visible and retry")), 2000);
            first = requestAnimationFrame(() => { second = requestAnimationFrame(() => finish()); });
        });
        if (document.querySelector(selector) !== video || video.currentSrc !== source) throw Error("Video source changed during frame capture");
    }
    const rect = video.getBoundingClientRect();
    let frameDataUrl = null;
    if (time !== null) {
        if (video.mediaKeys) throw Error("Protected video playback cannot provide decoded frames");
        try {
            const scale = Math.min(1, 1280 / video.videoWidth, 720 / video.videoHeight);
            const canvas = document.createElement("canvas");
            canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
            canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
            canvas.getContext("2d").drawImage(video, 0, 0, canvas.width, canvas.height);
            frameDataUrl = canvas.toDataURL("image/png");
        } catch (error) {
            if (error.name !== "SecurityError") throw error;
        }
    }
    const tracks = [...video.textTracks].slice(0, 8).map(track => ({ language: track.language, label: track.label, kind: track.kind,
        cues: [...(track.cues || [])].slice(0, 128).map(cue => ({ start: cue.startTime, end: cue.endTime, text: String(cue.text || "").slice(0, 1000) })) }));
    return { source, elementId, duration: Number.isFinite(video.duration) ? video.duration : null, currentTime: video.currentTime, width: video.videoWidth, height: video.videoHeight, paused: video.paused, readyState: video.readyState, tracks, frameDataUrl, captureMethod: frameDataUrl ? "decoded-frame" : "viewport",
        clip: { x: scrollX + Math.max(0, rect.left), y: scrollY + Math.max(0, rect.top), width: Math.max(0, Math.min(innerWidth, rect.right) - Math.max(0, rect.left)), height: Math.max(0, Math.min(innerHeight, rect.bottom) - Math.max(0, rect.top)), scale: 1 } };
}

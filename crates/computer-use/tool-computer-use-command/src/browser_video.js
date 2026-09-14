async function inspectVideo(selector, time) {
    const video = document.querySelector(selector);
    if (!(video instanceof HTMLVideoElement)) throw Error("No video element matches this selector; open a playable video page first.");
    if (video.readyState < 1) await new Promise((resolve, reject) => {
        const finish = error => { clearTimeout(timer); video.removeEventListener("loadedmetadata", ready); video.removeEventListener("error", failed); error ? reject(error) : resolve(); };
        const ready = () => finish(), failed = () => finish(Error("Video metadata could not be loaded"));
        const timer = setTimeout(() => finish(Error("Video metadata timed out")), 5000);
        video.addEventListener("loadedmetadata", ready); video.addEventListener("error", failed);
    });
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
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
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
    return { duration: Number.isFinite(video.duration) ? video.duration : null, currentTime: video.currentTime, width: video.videoWidth, height: video.videoHeight, paused: video.paused, readyState: video.readyState, tracks, frameDataUrl, captureMethod: frameDataUrl ? "decoded-frame" : "viewport",
        clip: { x: scrollX + Math.max(0, rect.left), y: scrollY + Math.max(0, rect.top), width: Math.max(0, Math.min(innerWidth, rect.right) - Math.max(0, rect.left)), height: Math.max(0, Math.min(innerHeight, rect.bottom) - Math.max(0, rect.top)), scale: 1 } };
}

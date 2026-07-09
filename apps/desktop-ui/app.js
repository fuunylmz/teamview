const FALLBACK_LOCAL_USER_ID = 101;

const fallbackClientApp = {
  role: "broadcaster",
  connection: "connected",
  relayAddr: "127.0.0.1:4433",
  selectedChannelId: 1,
  frame: 0,
  localVoice: {
    muted: false,
    deafened: false,
    pushToTalk: false,
    pttActive: false,
    inputLabel: "Default microphone",
    outputLabel: "Default speaker",
  },
  localScreenShare: {
    sharing: true,
    streamId: 1,
    sourceLabel: "Primary monitor",
    targetWidth: 1280,
    targetHeight: 720,
    targetFps: 30,
  },
  channels: [
    {
      channelId: 1,
      name: "General",
      participants: [
        participant(FALLBACK_LOCAL_USER_ID, "You", { speaking: true, sharingScreen: true }),
        participant(7, "Alice", { speaking: true }),
        participant(8, "Ben"),
        participant(9, "Chen", { muted: true }),
      ],
      screenStream: {
        streamId: 1,
        publisherId: FALLBACK_LOCAL_USER_ID,
        codec: "H264",
        title: "Primary monitor",
        width: 1280,
        height: 720,
        framesPerSecond: 30,
        subscribed: true,
        renderedFrames: 2840,
        droppedFrames: 0,
        latencyMs: 28,
        bitrateBps: 192000,
      },
      voiceStream: {
        streamId: 2,
        publisherId: FALLBACK_LOCAL_USER_ID,
        codec: "Opus",
        framesPerSecond: 50,
        subscribed: true,
        decodedFrames: 4680,
        droppedFrames: 0,
        latencyMs: 8,
        bitrateBps: 38400,
      },
    },
    {
      channelId: 2,
      name: "Ops Review",
      participants: [
        participant(FALLBACK_LOCAL_USER_ID, "You"),
        participant(10, "Dana", { speaking: true, sharingScreen: true }),
        participant(11, "Mina"),
      ],
      screenStream: {
        streamId: 4,
        publisherId: 10,
        codec: "H264",
        title: "Window capture",
        width: 1600,
        height: 900,
        framesPerSecond: 24,
        subscribed: true,
        renderedFrames: 1612,
        droppedFrames: 2,
        latencyMs: 54,
        bitrateBps: 307200,
      },
      voiceStream: {
        streamId: 5,
        publisherId: 10,
        codec: "Opus",
        framesPerSecond: 50,
        subscribed: true,
        decodedFrames: 2390,
        droppedFrames: 0,
        latencyMs: 11,
        bitrateBps: 38400,
      },
    },
    {
      channelId: 3,
      name: "Quiet Room",
      participants: [participant(FALLBACK_LOCAL_USER_ID, "You"), participant(12, "Noah")],
      screenStream: null,
      voiceStream: {
        streamId: 7,
        publisherId: 12,
        codec: "Opus",
        framesPerSecond: 50,
        subscribed: true,
        decodedFrames: 530,
        droppedFrames: 0,
        latencyMs: 9,
        bitrateBps: 38400,
      },
    },
  ],
};

let clientApp = cloneState(fallbackClientApp);
let localUserId = FALLBACK_LOCAL_USER_ID;

const elements = {
  channelList: document.querySelector("#channelList"),
  channelTitle: document.querySelector("#channelTitle"),
  channelMeta: document.querySelector("#channelMeta"),
  connectionState: document.querySelector("#connectionState"),
  relayState: document.querySelector("#relayState"),
  screenTitle: document.querySelector("#screenTitle"),
  screenPublisher: document.querySelector("#screenPublisher"),
  screenCodecMetric: document.querySelector("#screenCodecMetric"),
  resolutionMetric: document.querySelector("#resolutionMetric"),
  fpsMetric: document.querySelector("#fpsMetric"),
  latencyMetric: document.querySelector("#latencyMetric"),
  dropMetric: document.querySelector("#dropMetric"),
  shareButton: document.querySelector("#shareButton"),
  muteButton: document.querySelector("#muteButton"),
  deafenButton: document.querySelector("#deafenButton"),
  pttButton: document.querySelector("#pttButton"),
  voiceState: document.querySelector("#voiceState"),
  voiceDeviceState: document.querySelector("#voiceDeviceState"),
  voiceCodecMetric: document.querySelector("#voiceCodecMetric"),
  voiceLatencyMetric: document.querySelector("#voiceLatencyMetric"),
  voiceDropMetric: document.querySelector("#voiceDropMetric"),
  participantCount: document.querySelector("#participantCount"),
  participantList: document.querySelector("#participantList"),
  screenPreview: document.querySelector("#screenPreview"),
  voiceCanvas: document.querySelector("#voiceCanvas"),
};

function participant(userId, displayName, overrides = {}) {
  return {
    userId,
    displayName,
    muted: false,
    deafened: false,
    pushToTalk: false,
    speaking: false,
    sharingScreen: false,
    ...overrides,
  };
}

function cloneState(state) {
  return JSON.parse(JSON.stringify(state));
}

async function loadInitialState() {
  if (window.teamviewState) return window.teamviewState;
  const stateUrl = new URLSearchParams(window.location.search).get("state");
  if (!stateUrl) return null;

  try {
    const response = await fetch(stateUrl, { cache: "no-store" });
    if (!response.ok) return null;
    return await response.json();
  } catch {
    return null;
  }
}

function normalizeClientAppState(snapshot) {
  const fallback = cloneState(fallbackClientApp);
  const normalized = {
    ...fallback,
    ...snapshot,
    frame: 0,
    localVoice: {
      ...fallback.localVoice,
      ...(snapshot?.localVoice ?? {}),
    },
    localScreenShare: {
      ...fallback.localScreenShare,
      ...(snapshot?.localScreenShare ?? {}),
    },
    channels: Array.isArray(snapshot?.channels) && snapshot.channels.length > 0
      ? snapshot.channels.map(normalizeChannel)
      : fallback.channels.map(normalizeChannel),
  };
  normalized.selectedChannelId =
    normalized.selectedChannelId ?? normalized.channels[0]?.channelId ?? null;
  if (normalized.role !== "broadcaster") {
    normalized.localScreenShare.sharing = false;
  }
  return normalized;
}

function normalizeChannel(channel) {
  const participants = Array.isArray(channel.participants)
    ? channel.participants.map(normalizeParticipant)
    : [];
  return {
    channelId: channel.channelId,
    name: channel.name ?? `Channel ${channel.channelId}`,
    participantCount: channel.participantCount ?? participants.length,
    publishedStreamCount:
      channel.publishedStreamCount ??
      [channel.screenStream, channel.voiceStream].filter(Boolean).length,
    participants,
    screenStream: channel.screenStream ? normalizeScreenStream(channel.screenStream) : null,
    voiceStream: channel.voiceStream ? normalizeVoiceStream(channel.voiceStream) : null,
  };
}

function normalizeParticipant(entry) {
  return participant(entry.userId, entry.displayName ?? `User ${entry.userId}`, {
    muted: Boolean(entry.muted),
    deafened: Boolean(entry.deafened),
    pushToTalk: Boolean(entry.pushToTalk),
    speaking: Boolean(entry.speaking),
    sharingScreen: Boolean(entry.sharingScreen),
  });
}

function normalizeScreenStream(stream) {
  return {
    streamId: stream.streamId,
    publisherId: stream.publisherId,
    codec: stream.codec ?? "H264",
    title: stream.title ?? `Screen stream ${stream.streamId}`,
    width: stream.width ?? 0,
    height: stream.height ?? 0,
    framesPerSecond: stream.framesPerSecond ?? 0,
    subscribed: Boolean(stream.subscribed),
    renderedFrames: stream.renderedFrames ?? 0,
    droppedFrames: stream.droppedFrames ?? 0,
    latencyMs: stream.latencyMs ?? 0,
    bitrateBps: stream.bitrateBps ?? 0,
  };
}

function normalizeVoiceStream(stream) {
  return {
    streamId: stream.streamId,
    publisherId: stream.publisherId,
    codec: stream.codec ?? "Opus",
    framesPerSecond: stream.framesPerSecond ?? 0,
    subscribed: Boolean(stream.subscribed),
    decodedFrames: stream.decodedFrames ?? 0,
    droppedFrames: stream.droppedFrames ?? 0,
    latencyMs: stream.latencyMs ?? 0,
    bitrateBps: stream.bitrateBps ?? 0,
  };
}

function selectedChannel() {
  return (
    clientApp.channels.find((channel) => channel.channelId === clientApp.selectedChannelId) ??
    clientApp.channels[0] ??
    normalizeChannel({ channelId: 0, name: "No channels" })
  );
}

function render() {
  const channel = selectedChannel();
  if (channel.channelId !== clientApp.selectedChannelId) {
    clientApp.selectedChannelId = channel.channelId;
  }
  syncChannelState(channel);
  renderChannels(channel);
  renderTopbar(channel);
  renderScreenMetrics(channel);
  renderControls(channel);
  renderParticipants(channel);
  drawScreen(channel);
  drawVoice(channel);
}

function syncChannelState(channel) {
  for (const item of clientApp.channels) {
    const localParticipant = item.participants.find((entry) => entry.userId === localUserId);
    if (localParticipant && item.channelId !== channel.channelId) {
      localParticipant.speaking = false;
      localParticipant.sharingScreen = false;
    }
    if (item.channelId !== channel.channelId && item.screenStream?.publisherId === localUserId) {
      item.screenStream = null;
    }
    if (item.channelId !== channel.channelId && item.voiceStream?.publisherId === localUserId) {
      item.voiceStream = null;
    }
  }

  const localParticipant = ensureLocalParticipant(channel);
  localParticipant.muted = clientApp.localVoice.muted;
  localParticipant.deafened = clientApp.localVoice.deafened;
  localParticipant.pushToTalk = clientApp.localVoice.pushToTalk;
  localParticipant.speaking = localSpeaking();
  localParticipant.sharingScreen = clientApp.localScreenShare.sharing;

  if (clientApp.role === "broadcaster" && clientApp.localScreenShare.sharing) {
    const previous = channel.screenStream?.publisherId === localUserId ? channel.screenStream : {};
    channel.screenStream = {
      streamId: clientApp.localScreenShare.streamId,
      publisherId: localUserId,
      codec: "H264",
      title: clientApp.localScreenShare.sourceLabel,
      width: clientApp.localScreenShare.targetWidth,
      height: clientApp.localScreenShare.targetHeight,
      framesPerSecond: clientApp.localScreenShare.targetFps,
      subscribed: true,
      renderedFrames: previous.renderedFrames ?? 0,
      droppedFrames: previous.droppedFrames ?? 0,
      latencyMs: previous.latencyMs ?? 28,
      bitrateBps: 192000,
    };
  } else if (channel.screenStream?.publisherId === localUserId) {
    channel.screenStream = null;
  }

  if (clientApp.role === "broadcaster") {
    const previousVoice = channel.voiceStream?.publisherId === localUserId ? channel.voiceStream : {};
    channel.voiceStream = {
      streamId: previousVoice.streamId ?? 2,
      publisherId: localUserId,
      codec: "Opus",
      framesPerSecond: previousVoice.framesPerSecond ?? 50,
      subscribed: !clientApp.localVoice.deafened,
      decodedFrames: previousVoice.decodedFrames ?? 0,
      droppedFrames: previousVoice.droppedFrames ?? 0,
      latencyMs: previousVoice.latencyMs ?? 8,
      bitrateBps: previousVoice.bitrateBps ?? 38400,
    };
  } else if (channel.voiceStream?.publisherId === localUserId) {
    channel.voiceStream = null;
  }
}

function ensureLocalParticipant(channel) {
  let localParticipant = channel.participants.find((entry) => entry.userId === localUserId);
  if (!localParticipant) {
    localParticipant = participant(localUserId, "You");
    channel.participants.unshift(localParticipant);
  }
  return localParticipant;
}

function localSpeaking() {
  const voice = clientApp.localVoice;
  return !voice.muted && !voice.deafened && (!voice.pushToTalk || voice.pttActive);
}

function renderChannels(channel) {
  elements.channelList.replaceChildren(
    ...clientApp.channels.map((item) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = item.channelId === channel.channelId ? "channel active" : "channel";
      button.addEventListener("click", () => {
        clientApp.selectedChannelId = item.channelId;
        render();
      });

      const body = document.createElement("span");
      body.className = "channel-body";

      const name = document.createElement("span");
      name.className = "channel-name";
      name.textContent = item.name;

      const activity = document.createElement("span");
      activity.className = "channel-activity";
      activity.textContent = channelActivityLabel(item);

      const count = document.createElement("span");
      count.className = "channel-count";
      count.textContent = String(channelParticipantCount(item));

      body.append(name, activity);
      button.append(body, count);
      return button;
    }),
  );
}

function channelActivityLabel(channel) {
  const segments = [];
  if (channel.screenStream) segments.push("screen");
  const speakers = activeSpeakerCount(channel);
  if (speakers > 0) segments.push(`${speakers} voice`);
  if (segments.length === 0 && channel.publishedStreamCount > 0) {
    segments.push(`${channel.publishedStreamCount} streams`);
  }
  return segments.length > 0 ? segments.join(" / ") : "idle";
}

function renderTopbar(channel) {
  const speakers = activeSpeakerCount(channel);
  elements.channelTitle.textContent = channel.name;
  elements.channelMeta.textContent = `${channelParticipantCount(channel)} participants / ${speakers} speaking`;
  elements.connectionState.textContent = titleCase(clientApp.connection);
  elements.connectionState.dataset.status = clientApp.connection;
  elements.relayState.textContent = clientApp.relayAddr;
}

function renderScreenMetrics(channel) {
  const stream = channel.screenStream;
  elements.screenTitle.textContent = stream ? stream.title : "No screen stream";
  elements.screenPublisher.textContent = stream
    ? `Stream ${stream.streamId} / ${publisherName(channel, stream.publisherId)} / ${formatBitrate(stream.bitrateBps)}`
    : "Waiting for a publisher";
  elements.screenCodecMetric.textContent = stream ? stream.codec : "H264";
  elements.resolutionMetric.textContent = stream ? `${stream.width}x${stream.height}` : "0x0";
  elements.fpsMetric.textContent = stream ? `${stream.framesPerSecond} fps` : "0 fps";
  elements.latencyMetric.textContent = stream ? `${stream.latencyMs} ms` : "0 ms";
  elements.dropMetric.textContent = stream ? `${stream.droppedFrames} drops` : "0 drops";
}

function renderControls(channel) {
  const voice = clientApp.localVoice;
  elements.shareButton.classList.toggle("active", clientApp.localScreenShare.sharing);
  elements.muteButton.classList.toggle("danger", voice.muted);
  elements.deafenButton.classList.toggle("danger", voice.deafened);
  elements.pttButton.classList.toggle("active", voice.pushToTalk);
  elements.pttButton.disabled = voice.muted || voice.deafened;
  elements.shareButton.setAttribute("aria-pressed", String(clientApp.localScreenShare.sharing));
  elements.muteButton.setAttribute("aria-pressed", String(voice.muted));
  elements.deafenButton.setAttribute("aria-pressed", String(voice.deafened));
  elements.pttButton.setAttribute("aria-pressed", String(voice.pushToTalk));

  setButtonLabel(elements.shareButton, clientApp.localScreenShare.sharing ? "Stop share" : "Share screen");
  setButtonLabel(elements.muteButton, voice.muted ? "Unmute" : "Mute");
  setButtonLabel(elements.deafenButton, voice.deafened ? "Undeafen" : "Deafen");
  setButtonLabel(elements.pttButton, voice.pttActive ? "PTT active" : voice.pushToTalk ? "PTT ready" : "PTT");

  const voiceStream = channel.voiceStream;
  elements.voiceState.textContent = voiceLabel(channel);
  elements.voiceDeviceState.textContent = `${voice.inputLabel} / ${voice.outputLabel}`;
  elements.voiceCodecMetric.textContent = voiceStream
    ? `${voiceStream.codec} / ${voiceStream.framesPerSecond} fps`
    : "Opus / 0 fps";
  elements.voiceLatencyMetric.textContent = voiceStream ? `${voiceStream.latencyMs} ms` : "0 ms";
  elements.voiceDropMetric.textContent = voiceStream ? `${voiceStream.droppedFrames} drops` : "0 drops";
}

function setButtonLabel(button, label) {
  button.querySelector("span:last-child").textContent = label;
}

function voiceLabel(channel) {
  const voice = clientApp.localVoice;
  if (voice.deafened) return "Deafened";
  if (voice.muted) return "Muted";
  if (voice.pushToTalk && voice.pttActive) return "PTT active";
  if (voice.pushToTalk) return "PTT ready";
  return activeSpeakerCount(channel) > 0 ? "Voice active" : "Idle";
}

function renderParticipants(channel) {
  elements.participantCount.textContent = String(channelParticipantCount(channel));
  elements.participantList.replaceChildren(
    ...channel.participants.map((entry) => {
      const row = document.createElement("div");
      row.className = entry.speaking && !entry.muted ? "participant speaking" : "participant";

      const avatar = document.createElement("div");
      avatar.className = "avatar";
      avatar.textContent = initials(entry.displayName);

      const body = document.createElement("div");
      body.className = "participant-body";

      const name = document.createElement("div");
      name.className = "participant-name";
      name.textContent = entry.displayName;

      const meta = document.createElement("div");
      meta.className = "participant-meta";
      meta.textContent = participantMeta(entry);

      body.append(name, meta);

      const badges = document.createElement("div");
      badges.className = "participant-badges";
      for (const badge of participantBadges(entry)) {
        const node = document.createElement("span");
        node.className = `participant-badge ${badge.kind}`;
        node.textContent = badge.label;
        badges.append(node);
      }

      row.append(avatar, body, badges);
      return row;
    }),
  );
}

function participantMeta(entry) {
  const parts = [];
  if (entry.speaking && !entry.muted) parts.push("voice active");
  if (entry.sharingScreen) parts.push("screen live");
  if (entry.pushToTalk) parts.push("ptt");
  if (entry.muted) parts.push("muted");
  if (entry.deafened) parts.push("deafened");
  return parts.length > 0 ? parts.join(" / ") : "idle";
}

function participantBadges(entry) {
  const badges = [];
  if (entry.sharingScreen) badges.push({ kind: "screen", label: "screen" });
  if (entry.muted) badges.push({ kind: "muted", label: "muted" });
  if (entry.deafened) badges.push({ kind: "muted", label: "deaf" });
  if (!entry.muted && !entry.deafened) badges.push({ kind: "voice", label: "voice" });
  return badges;
}

function activeSpeakerCount(channel) {
  return channel.participants.filter((entry) => entry.speaking && !entry.muted).length;
}

function channelParticipantCount(channel) {
  return Math.max(channel.participantCount ?? 0, channel.participants.length);
}

function publisherName(channel, publisherId) {
  return channel.participants.find((entry) => entry.userId === publisherId)?.displayName ?? `User ${publisherId}`;
}

function formatBitrate(value) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} Mbps`;
  return `${Math.round(value / 1000)} Kbps`;
}

function titleCase(value) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function initials(name) {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0].toUpperCase())
    .join("");
}

function drawScreen(channel) {
  const canvas = elements.screenPreview;
  const ctx = canvas.getContext("2d");
  const width = canvas.width;
  const height = canvas.height;
  const stream = channel.screenStream;

  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = "#080a0d";
  ctx.fillRect(0, 0, width, height);

  if (!stream) {
    ctx.fillStyle = "#9aa4b2";
    ctx.font = "42px system-ui";
    ctx.fillText("No active stream", 64, 110);
    ctx.fillStyle = "#343b45";
    ctx.fillRect(64, 150, 360, 12);
    ctx.fillRect(64, 178, 270, 12);
    return;
  }

  const offset = clientApp.frame % 180;
  const gradient = ctx.createLinearGradient(0, 0, width, height);
  gradient.addColorStop(0, "#18222c");
  gradient.addColorStop(0.52, "#1d3532");
  gradient.addColorStop(1, "#202430");
  ctx.fillStyle = gradient;
  ctx.fillRect(0, 0, width, height);

  drawWindow(ctx, 72, 72, 500, 284, "#2cc6a4", offset);
  drawWindow(ctx, 620, 110, 420, 220, "#4d8dff", offset * 0.6);
  drawTimeline(ctx, 72, 430, width - 144, 120, offset);
  drawCursor(ctx, 860 + Math.sin(clientApp.frame / 18) * 160, 474 + Math.cos(clientApp.frame / 22) * 70);
}

function drawWindow(ctx, x, y, width, height, accent, offset) {
  ctx.fillStyle = "#151a20";
  ctx.strokeStyle = "#3b4652";
  ctx.lineWidth = 2;
  roundedRect(ctx, x, y, width, height, 8);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = accent;
  ctx.fillRect(x, y, width, 6);
  ctx.fillStyle = "#38424d";
  for (let index = 0; index < 6; index += 1) {
    const barWidth = 80 + ((offset + index * 23) % 180);
    ctx.fillRect(x + 28, y + 36 + index * 34, barWidth, 12);
  }
}

function drawTimeline(ctx, x, y, width, height, offset) {
  ctx.fillStyle = "#151a20";
  roundedRect(ctx, x, y, width, height, 8);
  ctx.fill();
  for (let index = 0; index < 18; index += 1) {
    const size = 26 + ((offset + index * 11) % 54);
    ctx.fillStyle = index % 3 === 0 ? "#2cc6a4" : index % 3 === 1 ? "#4d8dff" : "#e6a93c";
    ctx.fillRect(x + 28 + index * 58, y + height - size - 20, 32, size);
  }
}

function drawCursor(ctx, x, y) {
  ctx.fillStyle = "#f2f4f8";
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + 18, y + 42);
  ctx.lineTo(x + 28, y + 26);
  ctx.lineTo(x + 46, y + 24);
  ctx.closePath();
  ctx.fill();
}

function roundedRect(ctx, x, y, width, height, radius) {
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + width, y, x + width, y + height, radius);
  ctx.arcTo(x + width, y + height, x, y + height, radius);
  ctx.arcTo(x, y + height, x, y, radius);
  ctx.arcTo(x, y, x + width, y, radius);
  ctx.closePath();
}

function drawVoice(channel) {
  const canvas = elements.voiceCanvas;
  const ctx = canvas.getContext("2d");
  const width = canvas.width;
  const height = canvas.height;
  const activeSpeakers = activeSpeakerCount(channel);

  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = "#11161b";
  ctx.fillRect(0, 0, width, height);
  for (let index = 0; index < 36; index += 1) {
    const wave = Math.sin((clientApp.frame + index * 9) / 9);
    const gain = clientApp.localVoice.muted || clientApp.localVoice.deafened ? 0.18 : 0.48 + activeSpeakers * 0.12;
    const barHeight = Math.max(5, Math.abs(wave) * height * gain);
    ctx.fillStyle = index % 4 === 0 ? "#2cc6a4" : "#4d8dff";
    ctx.fillRect(index * 10, (height - barHeight) / 2, 5, barHeight);
  }
}

elements.shareButton.addEventListener("click", () => {
  clientApp.localScreenShare.sharing = !clientApp.localScreenShare.sharing;
  render();
});

elements.muteButton.addEventListener("click", () => {
  clientApp.localVoice.muted = !clientApp.localVoice.muted;
  if (!clientApp.localVoice.muted) clientApp.localVoice.deafened = false;
  if (clientApp.localVoice.muted) clientApp.localVoice.pttActive = false;
  render();
});

elements.deafenButton.addEventListener("click", () => {
  clientApp.localVoice.deafened = !clientApp.localVoice.deafened;
  if (clientApp.localVoice.deafened) {
    clientApp.localVoice.muted = true;
    clientApp.localVoice.pttActive = false;
  }
  render();
});

elements.pttButton.addEventListener("click", () => {
  if (clientApp.localVoice.muted || clientApp.localVoice.deafened) return;
  clientApp.localVoice.pushToTalk = !clientApp.localVoice.pushToTalk;
  clientApp.localVoice.pttActive = clientApp.localVoice.pushToTalk;
  render();
});

function animate() {
  clientApp.frame += 1;
  drawScreen(selectedChannel());
  drawVoice(selectedChannel());
  requestAnimationFrame(animate);
}

async function boot() {
  const loadedState = await loadInitialState();
  clientApp = normalizeClientAppState(loadedState ?? fallbackClientApp);
  localUserId = clientApp.localUserId ?? FALLBACK_LOCAL_USER_ID;
  render();
  animate();
}

boot();

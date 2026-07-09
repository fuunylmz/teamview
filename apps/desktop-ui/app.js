const appState = {
  selectedChannelId: 1,
  connected: true,
  sharing: true,
  muted: false,
  deafened: false,
  pushToTalk: false,
  frame: 0,
  channels: [
    {
      id: 1,
      name: "General",
      screen: { title: "Primary monitor", width: 1280, height: 720, fps: 30, latency: 42 },
      participants: [
        { id: 7, name: "Alice", speaking: true, muted: false, screen: true },
        { id: 8, name: "Ben", speaking: false, muted: false, screen: false },
        { id: 9, name: "Chen", speaking: false, muted: true, screen: false },
      ],
    },
    {
      id: 2,
      name: "Ops Review",
      screen: { title: "Window capture", width: 1600, height: 900, fps: 24, latency: 58 },
      participants: [
        { id: 10, name: "Dana", speaking: true, muted: false, screen: true },
        { id: 11, name: "Mina", speaking: false, muted: false, screen: false },
      ],
    },
    {
      id: 3,
      name: "Quiet Room",
      screen: null,
      participants: [{ id: 12, name: "Noah", speaking: false, muted: false, screen: false }],
    },
  ],
};

const elements = {
  channelList: document.querySelector("#channelList"),
  channelTitle: document.querySelector("#channelTitle"),
  connectionState: document.querySelector("#connectionState"),
  screenTitle: document.querySelector("#screenTitle"),
  resolutionMetric: document.querySelector("#resolutionMetric"),
  fpsMetric: document.querySelector("#fpsMetric"),
  latencyMetric: document.querySelector("#latencyMetric"),
  shareButton: document.querySelector("#shareButton"),
  muteButton: document.querySelector("#muteButton"),
  deafenButton: document.querySelector("#deafenButton"),
  pttButton: document.querySelector("#pttButton"),
  voiceState: document.querySelector("#voiceState"),
  participantCount: document.querySelector("#participantCount"),
  participantList: document.querySelector("#participantList"),
  screenPreview: document.querySelector("#screenPreview"),
  voiceCanvas: document.querySelector("#voiceCanvas"),
};

function selectedChannel() {
  return appState.channels.find((channel) => channel.id === appState.selectedChannelId);
}

function render() {
  const channel = selectedChannel();
  renderChannels(channel);
  renderTopbar(channel);
  renderControls();
  renderParticipants(channel);
  drawScreen(channel);
  drawVoice(channel);
}

function renderChannels(channel) {
  elements.channelList.replaceChildren(
    ...appState.channels.map((item) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = item.id === channel.id ? "channel active" : "channel";
      button.addEventListener("click", () => {
        appState.selectedChannelId = item.id;
        render();
      });

      const name = document.createElement("span");
      name.className = "channel-name";
      name.textContent = item.name;

      const count = document.createElement("span");
      count.className = "channel-count";
      count.textContent = String(item.participants.length);

      button.append(name, count);
      return button;
    }),
  );
}

function renderTopbar(channel) {
  const screen = channel.screen;
  elements.channelTitle.textContent = channel.name;
  elements.connectionState.textContent = appState.connected ? "Connected" : "Offline";
  elements.screenTitle.textContent = screen ? screen.title : "No screen stream";
  elements.resolutionMetric.textContent = screen ? `${screen.width}x${screen.height}` : "0x0";
  elements.fpsMetric.textContent = screen ? `${screen.fps} fps` : "0 fps";
  elements.latencyMetric.textContent = screen ? `${screen.latency} ms` : "0 ms";
}

function renderControls() {
  elements.shareButton.classList.toggle("active", appState.sharing);
  elements.muteButton.classList.toggle("danger", appState.muted);
  elements.deafenButton.classList.toggle("danger", appState.deafened);
  elements.pttButton.classList.toggle("active", appState.pushToTalk);
  elements.voiceState.textContent = voiceLabel();
}

function voiceLabel() {
  if (appState.deafened) return "Deafened";
  if (appState.muted) return "Muted";
  if (appState.pushToTalk) return "PTT armed";
  return "Speaking";
}

function renderParticipants(channel) {
  elements.participantCount.textContent = String(channel.participants.length);
  elements.participantList.replaceChildren(
    ...channel.participants.map((participant) => {
      const row = document.createElement("div");
      row.className = participant.speaking ? "participant speaking" : "participant";

      const avatar = document.createElement("div");
      avatar.className = "avatar";
      avatar.textContent = initials(participant.name);

      const body = document.createElement("div");
      const name = document.createElement("div");
      name.className = "participant-name";
      name.textContent = participant.name;
      const meta = document.createElement("div");
      meta.className = "participant-meta";
      meta.textContent = participant.speaking ? "voice active" : "idle";
      body.append(name, meta);

      const badge = document.createElement("span");
      if (participant.screen) {
        badge.className = "participant-badge screen";
        badge.textContent = "screen";
      } else if (participant.muted) {
        badge.className = "participant-badge muted";
        badge.textContent = "muted";
      } else {
        badge.className = "participant-badge";
        badge.textContent = "voice";
      }

      row.append(avatar, body, badge);
      return row;
    }),
  );
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
  const screen = channel.screen;

  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = "#080a0d";
  ctx.fillRect(0, 0, width, height);

  if (!screen) {
    ctx.fillStyle = "#9aa4b2";
    ctx.font = "42px system-ui";
    ctx.fillText("No active stream", 64, 110);
    return;
  }

  const offset = appState.frame % 180;
  const gradient = ctx.createLinearGradient(0, 0, width, height);
  gradient.addColorStop(0, "#1e2934");
  gradient.addColorStop(0.5, "#243a3b");
  gradient.addColorStop(1, "#1b2130");
  ctx.fillStyle = gradient;
  ctx.fillRect(0, 0, width, height);

  drawWindow(ctx, 72, 72, 500, 284, "#2cc6a4", offset);
  drawWindow(ctx, 620, 110, 420, 220, "#4d8dff", offset * 0.6);
  drawTimeline(ctx, 72, 430, width - 144, 120, offset);
  drawCursor(ctx, 860 + Math.sin(appState.frame / 18) * 160, 474 + Math.cos(appState.frame / 22) * 70);
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
  const activeSpeakers = channel.participants.filter((participant) => participant.speaking).length;

  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = "#11161b";
  ctx.fillRect(0, 0, width, height);
  for (let index = 0; index < 36; index += 1) {
    const wave = Math.sin((appState.frame + index * 9) / 9);
    const gain = appState.muted || appState.deafened ? 0.2 : 0.55 + activeSpeakers * 0.14;
    const barHeight = Math.max(5, Math.abs(wave) * height * gain);
    ctx.fillStyle = index % 4 === 0 ? "#2cc6a4" : "#4d8dff";
    ctx.fillRect(index * 10, (height - barHeight) / 2, 5, barHeight);
  }
}

elements.shareButton.addEventListener("click", () => {
  appState.sharing = !appState.sharing;
  render();
});

elements.muteButton.addEventListener("click", () => {
  appState.muted = !appState.muted;
  if (appState.muted) appState.pushToTalk = false;
  render();
});

elements.deafenButton.addEventListener("click", () => {
  appState.deafened = !appState.deafened;
  if (appState.deafened) appState.muted = true;
  render();
});

elements.pttButton.addEventListener("click", () => {
  if (!appState.muted && !appState.deafened) {
    appState.pushToTalk = !appState.pushToTalk;
  }
  render();
});

function animate() {
  appState.frame += 1;
  drawScreen(selectedChannel());
  drawVoice(selectedChannel());
  requestAnimationFrame(animate);
}

render();
animate();

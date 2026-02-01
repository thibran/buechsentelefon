import { LOBBY, MAX_RECONNECT_ATTEMPTS } from './constants.js';

export class RtcManager {
    constructor(onStateChangeCallback) {
        this.ws = null;
        this.localStream = null;
        this.analysisStream = null;
        this.previewStream = null;
        this.peers = {};
        this.currentUser = null;
        this.currentRoom = LOBBY;
        this.isMuted = false;
        this.onStateChange = onStateChangeCallback;

        this.micThreshold = 10;
        this.masterVolume = 1.0;
        this.rtcConfig = { iceServers: [] };

        this.audioContext = new (window.AudioContext || window.webkitAudioContext)();
        this.lastSpokeTime = 0;
        this.loopbackEnabled = false;
        this.localAudioNodes = null;
        this.previewAudioNodes = null;
        this.remoteAudioNodes = {};
        this._localPollId = null;
        this._previewPollId = null;

        this.micGain = 1.0;
        this.micGainNode = null;
        this.micGainPipeline = null;
        this.rawStream = null;

        this.echoCancellation = true;
        this.noiseSuppression = true;
        this.autoGainControl = true;

        this.loopbackPC1 = null;
        this.loopbackPC2 = null;

        this.reconnectAttempts = 0;
        this.intentionalClose = false;
        this.lastRoom = null;
        this.lastUserName = null;

        this.loopbackEl = document.createElement('audio');
        this.audioContainer = document.createElement('div');
        this.audioContainer.style.display = 'none';
        document.body.appendChild(this.audioContainer);
    }

    setStunServers(servers) {
        this.rtcConfig = {
            iceServers: servers.map(s => ({ urls: s }))
        };
    }

    async getDevices() {
        try {
            const tempStream = await navigator.mediaDevices.getUserMedia({ audio: true });
            const devices = await navigator.mediaDevices.enumerateDevices();
            tempStream.getTracks().forEach(t => t.stop());
            return devices;
        } catch (e) {
            return [];
        }
    }

    setUser(user) {
        this.currentUser = user;
        if (user.settings) {
            this.micThreshold = user.settings.micThreshold || 10;
            this.masterVolume = (user.settings.masterVolume !== undefined)
                ? user.settings.masterVolume : 1.0;
            this.micGain = (user.settings.micGain !== undefined)
                ? user.settings.micGain : 1.0;
            if (user.settings.echoCancellation !== undefined)
                this.echoCancellation = user.settings.echoCancellation;
            if (user.settings.noiseSuppression !== undefined)
                this.noiseSuppression = user.settings.noiseSuppression;
            if (user.settings.autoGainControl !== undefined)
                this.autoGainControl = user.settings.autoGainControl;
        }
        if (this.micGainNode) {
            this.micGainNode.gain.value = this.micGain;
        }
        this.updateAllVolumes();
    }

    connect() {
        if (this.ws) return;
        this.intentionalClose = false;

        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        this.ws = new WebSocket(`${protocol}//${window.location.host}/ws`);

        this.ws.onopen = () => {
            this.reconnectAttempts = 0;
            this.onStateChange('connection-status', 'connected');

            if (this.lastRoom && this.lastRoom !== LOBBY && this.lastUserName) {
                this.sendJoin(this.lastRoom);
            } else {
                this.sendJoin(LOBBY);
            }
        };

        this.ws.onmessage = async (e) => {
            await this.handleMessage(JSON.parse(e.data));
        };

        this.ws.onclose = () => {
            this.ws = null;
            if (!this.intentionalClose) {
                this.onStateChange('connection-status', 'disconnected');
                this.attemptReconnect();
            }
        };

        this.ws.onerror = () => {
            this.onStateChange('connection-status', 'error');
        };
    }

    disconnect() {
        this.intentionalClose = true;
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
    }

    attemptReconnect() {
        if (this.reconnectAttempts >= MAX_RECONNECT_ATTEMPTS) {
            this.onStateChange('connection-status', 'failed');
            return;
        }

        const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts), 30000);
        this.reconnectAttempts++;
        this.onStateChange('connection-status', 'reconnecting');

        setTimeout(() => {
            this.connect();
        }, delay);
    }

    sendJoin(roomName, password) {
        if (!this.ws || !this.currentUser) return;
        this.closeAllPeers();

        const msg = {
            type: 'join',
            room: roomName,
            name: this.currentUser.name
        };
        if (password) msg.password = password;

        this.ws.send(JSON.stringify(msg));

        this.lastRoom = roomName;
        this.lastUserName = this.currentUser.name;
    }

    _getAudioConstraints(deviceId) {
        return {
            audio: {
                deviceId: deviceId ? { exact: deviceId } : undefined,
                echoCancellation: this.echoCancellation,
                noiseSuppression: this.noiseSuppression,
                autoGainControl: this.autoGainControl
            },
            video: false
        };
    }

    _createMicGainPipeline(rawStream) {
        const source = this.audioContext.createMediaStreamSource(rawStream);
        const gainNode = this.audioContext.createGain();
        gainNode.gain.value = this.micGain;
        const destination = this.audioContext.createMediaStreamDestination();
        source.connect(gainNode);
        gainNode.connect(destination);
        this.micGainNode = gainNode;
        this.micGainPipeline = { source, gainNode, destination };
        return destination.stream;
    }

    _destroyMicGainPipeline() {
        if (this.micGainPipeline) {
            try {
                this.micGainPipeline.source.disconnect();
                this.micGainPipeline.gainNode.disconnect();
            } catch (e) { /* ignore */ }
            this.micGainPipeline = null;
            this.micGainNode = null;
        }
    }

    setMicGain(value) {
        this.micGain = value;
        if (this.micGainNode) {
            this.micGainNode.gain.value = value;
        }
    }

    async startMic() {
        try {
            if (this.audioContext.state === 'suspended') {
                await this.audioContext.resume();
            }
            if (this.localStream) return;

            const constraints = this._getAudioConstraints(this.currentUser?.audioInput);
            this.rawStream = await navigator.mediaDevices.getUserMedia(constraints);
            this.analysisStream = this.rawStream.clone();
            this.localStream = this._createMicGainPipeline(this.rawStream);
            this.applyMicState();
            this.setupLocalAnalyser();
        } catch (e) {
            console.error("Failed to start microphone:", e);
        }
    }

    async startPreview(deviceId) {
        this.stopPreview();

        try {
            if (this.audioContext.state === 'suspended') {
                await this.audioContext.resume();
            }

            const constraints = this._getAudioConstraints(deviceId);
            this.previewStream = await navigator.mediaDevices.getUserMedia(constraints);
            this.setupPreviewAnalyser();

            if (this.loopbackEnabled) {
                await this.playLoopback();
            }
        } catch (e) {
            console.error("Failed to start preview:", e);
        }
    }

    stopPreview() {
        if (this._previewPollId) {
            clearInterval(this._previewPollId);
            this._previewPollId = null;
        }
        this._closeLoopbackPCs();
        if (this.previewStream) {
            this.previewStream.getTracks().forEach(t => t.stop());
            this.previewStream = null;
        }
        if (this.previewAudioNodes) {
            try {
                this.previewAudioNodes.source.disconnect();
                this.previewAudioNodes.analyser.disconnect();
                this.previewAudioNodes.silencer.disconnect();
            } catch (e) { /* ignore */ }
            this.previewAudioNodes = null;
        }
        this.loopbackEl.srcObject = null;
    }

    stopMic() {
        if (this._localPollId) {
            clearInterval(this._localPollId);
            this._localPollId = null;
        }
        this._destroyMicGainPipeline();
        if (this.rawStream) {
            this.rawStream.getTracks().forEach(t => t.stop());
            this.rawStream = null;
        }
        this.localStream = null;
        if (this.analysisStream) {
            this.analysisStream.getTracks().forEach(t => t.stop());
            this.analysisStream = null;
        }
        if (this.localAudioNodes) {
            try {
                this.localAudioNodes.source.disconnect();
                this.localAudioNodes.analyser.disconnect();
                this.localAudioNodes.silencer.disconnect();
            } catch (e) { /* ignore */ }
            this.localAudioNodes = null;
        }
    }

    toggleMute() {
        this.isMuted = !this.isMuted;
        this.applyMicState();
        return this.isMuted;
    }

    applyMicState() {
        if (this.localStream) {
            this.localStream.getAudioTracks().forEach(t => t.enabled = !this.isMuted);
        }
    }

    setLoopback(enabled) {
        this.loopbackEnabled = enabled;
        if (enabled && this.previewStream) {
            this.playLoopback();
        } else {
            this._closeLoopbackPCs();
            this.loopbackEl.srcObject = null;
        }
    }

    _closeLoopbackPCs() {
        if (this.loopbackPC1) {
            this.loopbackPC1.close();
            this.loopbackPC1 = null;
        }
        if (this.loopbackPC2) {
            this.loopbackPC2.close();
            this.loopbackPC2 = null;
        }
    }

    async playLoopback() {
        this._closeLoopbackPCs();

        this.loopbackPC1 = new RTCPeerConnection();
        this.loopbackPC2 = new RTCPeerConnection();

        this.loopbackPC1.onicecandidate = e => {
            if (e.candidate) this.loopbackPC2.addIceCandidate(e.candidate);
        };
        this.loopbackPC2.onicecandidate = e => {
            if (e.candidate) this.loopbackPC1.addIceCandidate(e.candidate);
        };

        this.previewStream.getTracks().forEach(t =>
            this.loopbackPC1.addTrack(t, this.previewStream)
        );

        this.loopbackPC2.ontrack = (e) => {
            this.loopbackEl.srcObject = e.streams[0];
            if (this.loopbackEl.setSinkId && this.currentUser?.audioOutput) {
                this.loopbackEl.setSinkId(this.currentUser.audioOutput).catch(() => {});
            }
            this.loopbackEl.volume = this.masterVolume;
            this.loopbackEl.play().catch(() => {});
        };

        const offer = await this.loopbackPC1.createOffer();
        await this.loopbackPC1.setLocalDescription(offer);
        await this.loopbackPC2.setRemoteDescription(offer);
        const answer = await this.loopbackPC2.createAnswer();
        await this.loopbackPC2.setLocalDescription(answer);
        await this.loopbackPC1.setRemoteDescription(answer);
    }

    async applyDeviceChange(audioInput, audioOutput) {
        if (!this.localStream) return;

        const constraints = this._getAudioConstraints(audioInput);
        const newRawStream = await navigator.mediaDevices.getUserMedia(constraints);

        this._destroyMicGainPipeline();
        const newGainedStream = this._createMicGainPipeline(newRawStream);
        const newTrack = newGainedStream.getAudioTracks()[0];
        const oldTrack = this.localStream.getAudioTracks()[0];

        for (const peerId in this.peers) {
            const peer = this.peers[peerId];
            const senders = peer.pc.getSenders();
            const audioSender = senders.find(s => s.track?.kind === 'audio' || s.track === oldTrack);
            if (audioSender) {
                await audioSender.replaceTrack(newTrack);
            }
        }

        if (this.rawStream) {
            this.rawStream.getTracks().forEach(t => t.stop());
        }
        this.rawStream = newRawStream;
        this.localStream = newGainedStream;
        this.applyMicState();

        if (this._localPollId) {
            clearInterval(this._localPollId);
            this._localPollId = null;
        }
        if (this.analysisStream) {
            this.analysisStream.getTracks().forEach(t => t.stop());
        }
        if (this.localAudioNodes) {
            try {
                this.localAudioNodes.source.disconnect();
                this.localAudioNodes.analyser.disconnect();
                this.localAudioNodes.silencer.disconnect();
            } catch (e) { /* ignore */ }
            this.localAudioNodes = null;
        }
        this.analysisStream = this.rawStream.clone();
        this.setupLocalAnalyser();

        if (audioOutput) {
            for (const peerId in this.peers) {
                const el = document.getElementById(`audio-${peerId}`);
                if (el?.setSinkId) {
                    el.setSinkId(audioOutput).catch(() => {});
                }
            }
        }
    }

    setPeerVolume(id, vol) {
        const audioEl = document.getElementById(`audio-${id}`);
        if (audioEl) audioEl.volume = vol * this.masterVolume;
    }

    updateAllVolumes() {
        if (this.loopbackEl) this.loopbackEl.volume = this.masterVolume;
        this.onStateChange('request-volume-refresh', null);
    }

    async handleMessage(msg) {
        switch (msg.type) {
            case 'state-update':
                if (this.currentRoom !== LOBBY) {
                    const validIds = (msg.rooms[this.currentRoom] || []).map(u => u.id);
                    Object.keys(this.peers).forEach(id => {
                        if (!validIds.includes(id)) this.closePeer(id);
                    });
                }
                this.onStateChange('state-update', msg.rooms);
                break;

            case 'you-joined':
                this.currentRoom = msg.room;
                this.onStateChange('you-joined', this.currentRoom);
                if (this.currentRoom === LOBBY) {
                    this.stopMic();
                } else {
                    await this.startMic();
                }
                break;

            case 'peer-joined':
                if (this.currentRoom !== LOBBY) {
                    await this.createPeer(msg.id, true);
                }
                break;

            case 'offer':
                await this.handleOffer(msg.src, msg.sdp);
                break;

            case 'answer':
                await this.handleAnswer(msg.src, msg.sdp);
                break;

            case 'candidate':
                await this.handleCandidate(msg.src, msg.candidate);
                break;

            case 'error':
                this.onStateChange('server-error', msg.message);
                break;
        }
    }

    async createPeer(targetId, isReceiver) {
        if (this.peers[targetId]) return;

        const pc = new RTCPeerConnection(this.rtcConfig);

        if (this.localStream) {
            this.localStream.getTracks().forEach(t => pc.addTrack(t, this.localStream));
            const audioSender = pc.getSenders().find(s => s.track?.kind === 'audio');
            if (audioSender) {
                try {
                    const params = audioSender.getParameters();
                    if (!params.encodings || params.encodings.length === 0) {
                        params.encodings = [{}];
                    }
                    params.encodings[0].maxBitrate = 128000;
                    audioSender.setParameters(params).catch(() => {});
                } catch (e) { /* browser may not support setParameters */ }
            }
        }

        pc.onicecandidate = (e) => {
            if (e.candidate) {
                this.ws.send(JSON.stringify({
                    type: 'candidate',
                    target: targetId,
                    candidate: e.candidate
                }));
            }
        };

        pc.onconnectionstatechange = () => {
            this.onStateChange('peer-connection-state', {
                id: targetId,
                state: pc.connectionState
            });
        };

        pc.ontrack = (e) => {
            let el = document.getElementById(`audio-${targetId}`);
            if (!el) {
                el = document.createElement('audio');
                el.id = `audio-${targetId}`;
                this.audioContainer.appendChild(el);
            }
            el.srcObject = e.streams[0];
            el.play().catch(() => {});
            if (el.setSinkId && this.currentUser?.audioOutput) {
                el.setSinkId(this.currentUser.audioOutput).catch(() => {});
            }
            el.volume = this.masterVolume;
            this.setupRemoteAnalyser(e.streams[0], targetId);
            this.onStateChange('peer-connected', targetId);
        };

        this.peers[targetId] = { pc };

        if (!isReceiver) {
            const offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            this.ws.send(JSON.stringify({
                type: 'offer',
                target: targetId,
                sdp: offer
            }));
        }
    }

    async handleOffer(srcId, sdp) {
        let peer = this.peers[srcId];
        if (!peer) {
            await this.createPeer(srcId, true);
            peer = this.peers[srcId];
        }
        await peer.pc.setRemoteDescription(new RTCSessionDescription(sdp));
        const answer = await peer.pc.createAnswer();
        await peer.pc.setLocalDescription(answer);
        this.ws.send(JSON.stringify({ type: 'answer', target: srcId, sdp: answer }));
    }

    async handleAnswer(srcId, sdp) {
        if (this.peers[srcId]) {
            await this.peers[srcId].pc.setRemoteDescription(new RTCSessionDescription(sdp));
        }
    }

    async handleCandidate(srcId, cand) {
        if (this.peers[srcId]) {
            await this.peers[srcId].pc.addIceCandidate(new RTCIceCandidate(cand));
        }
    }

    closePeer(id) {
        if (this.peers[id]) {
            this.peers[id].pc.close();
            delete this.peers[id];
        }
        const el = document.getElementById(`audio-${id}`);
        if (el) el.remove();
        if (this.remoteAudioNodes[id]) {
            try {
                this.remoteAudioNodes[id].source.disconnect();
                this.remoteAudioNodes[id].analyser.disconnect();
                this.remoteAudioNodes[id].silencer.disconnect();
                clearInterval(this.remoteAudioNodes[id].pollId);
            } catch (e) { /* ignore */ }
            delete this.remoteAudioNodes[id];
        }
    }

    closeAllPeers() {
        Object.keys(this.peers).forEach(id => this.closePeer(id));
    }

    // --- Audio Analysis (no ScriptProcessorNode, no latency) ---

    _createAnalyserChain(stream) {
        const source = this.audioContext.createMediaStreamSource(stream);
        const analyser = this.audioContext.createAnalyser();
        const silencer = this.audioContext.createGain();
        silencer.gain.value = 0;

        analyser.smoothingTimeConstant = 0.3;
        analyser.fftSize = 1024;

        source.connect(analyser);
        analyser.connect(silencer);
        silencer.connect(this.audioContext.destination);

        return { source, analyser, silencer };
    }

    _readLevel(analyser) {
        const array = new Uint8Array(analyser.frequencyBinCount);
        analyser.getByteFrequencyData(array);
        let values = 0;
        for (let i = 0; i < array.length; i++) values += array[i];
        const raw = (values / array.length) / 255;
        return Math.pow(raw, 0.4) * 100;
    }

    setupLocalAnalyser() {
        this.localAudioNodes = this._createAnalyserChain(this.analysisStream);

        this._localPollId = setInterval(() => {
            const average = this._readLevel(this.localAudioNodes.analyser);

            if (this.localStream) {
                const threshold = this.micThreshold;
                if (!this.isMuted && threshold > 0) {
                    if (average > threshold) {
                        this.lastSpokeTime = Date.now();
                        this.localStream.getAudioTracks().forEach(t => t.enabled = true);
                        this.onStateChange('gate-activity', true);
                    } else if (Date.now() - this.lastSpokeTime > 500) {
                        this.localStream.getAudioTracks().forEach(t => t.enabled = false);
                        this.onStateChange('gate-activity', false);
                    } else {
                        this.onStateChange('gate-activity', true);
                    }
                } else if (this.isMuted) {
                    this.onStateChange('gate-activity', false);
                } else {
                    this.onStateChange('gate-activity', true);
                }
            }

            this.onStateChange('audio-level', { id: 'local', level: average });
        }, 50);
    }

    setupPreviewAnalyser() {
        this.previewAudioNodes = this._createAnalyserChain(this.previewStream);
        this._previewLastSpoke = 0;

        this._previewPollId = setInterval(() => {
            const average = this._readLevel(this.previewAudioNodes.analyser);

            if (this.loopbackEnabled && this.loopbackEl.srcObject) {
                const threshold = this.micThreshold;
                if (threshold > 0) {
                    if (average > threshold) {
                        this._previewLastSpoke = Date.now();
                        this.loopbackEl.volume = this.masterVolume;
                    } else if (Date.now() - this._previewLastSpoke > 500) {
                        this.loopbackEl.volume = 0;
                    }
                } else {
                    this.loopbackEl.volume = this.masterVolume;
                }
            }

            this.onStateChange('audio-level', { id: 'preview', level: average });
        }, 50);
    }

    setupRemoteAnalyser(stream, id) {
        const nodes = this._createAnalyserChain(stream);
        const pollId = setInterval(() => {
            const average = this._readLevel(nodes.analyser);
            this.onStateChange('audio-level', { id, level: average });
        }, 50);
        this.remoteAudioNodes[id] = { ...nodes, pollId };
    }
}

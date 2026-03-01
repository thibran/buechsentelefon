import * as API from './api.js';
import { RtcManager } from './rtc.js';
import { LOBBY, STORAGE_KEY_USER, STORAGE_KEY_PEERS, STORAGE_KEY_THEME } from './constants.js';
import { detectLanguage, translate, STORAGE_KEY_LANG } from './i18n.js';

let rtc;

function getPreferredTheme() {
    const stored = localStorage.getItem(STORAGE_KEY_THEME);
    if (stored) return stored;
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function applyTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem(STORAGE_KEY_THEME, theme);
}

const buechseAppComponent = () => ({
    config: { title: "Loading...", stun_servers: [], branding: {}, legal: {}, has_users: false },
    authenticated: false,
    usernameInput: '',
    passwordInput: '',
    loginError: '',
    userRole: 'standard',
    sessionUsername: null,
    currentUser: null,
    showSettings: false,
    showLegalModal: false,
    legalTitle: '',
    legalContent: '',
    rooms: [],
    roomState: {},
    currentRoom: LOBBY,
    isMuted: false,
    peersSettings: {},
    peerStates: {},
    connectionStatus: 'disconnected',
    settingsForm: {
        name: '',
        audioInput: '',
        audioOutput: '',
        micThreshold: 10,
        masterVolume: 1.0,
        micGain: 1.0,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true
    },
    devices: { inputs: [], outputs: [] },
    loopbackEnabled: false,
    isGateOpen: false,
    darkMode: getPreferredTheme() === 'dark',
    lang: detectLanguage(),

    roomPasswordPrompt: null,
    roomPasswordInput: '',

    t(key) {
        return translate(this.lang, key);
    },

    setLang(newLang) {
        this.lang = newLang;
        localStorage.setItem(STORAGE_KEY_LANG, newLang);
        document.documentElement.lang = newLang;
    },

    async initApp() {
        document.documentElement.lang = this.lang;
        applyTheme(this.darkMode ? 'dark' : 'light');

        try {
            this.config = await API.fetchConfig();
            document.title = this.config.title;
        } catch (e) { /* ignore */ }

        this.applyBranding();
        this.loadUser();

        rtc = new RtcManager((event, data) => this.handleRtcEvent(event, data));

        if (this.config.stun_servers?.length) {
            rtc.setStunServers(this.config.stun_servers);
        }

        const authInfo = await API.checkAuth();
        if (authInfo) {
            this.authenticated = true;
            this.userRole = authInfo.role || 'standard';
            this.sessionUsername = authInfo.username || null;
            this.startSession();
        }

        this.setupKeyboardShortcuts();
    },

    setupKeyboardShortcuts() {
        document.addEventListener('keydown', (e) => {
            const tag = document.activeElement?.tagName?.toLowerCase();
            if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
            if (!this.authenticated) return;

            if (e.key === 'm' || e.key === 'M') {
                e.preventDefault();
                if (this.currentRoom !== LOBBY) {
                    this.toggleMute();
                }
            }

            if (e.key === 'Escape') {
                if (this.showLegalModal) {
                    this.closeLegal();
                } else if (this.roomPasswordPrompt) {
                    this.roomPasswordPrompt = null;
                    this.roomPasswordInput = '';
                } else if (this.showSettings && this.currentUser?.name) {
                    this.saveSettings();
                }
            }
        });
    },

    applyBranding() {
        const b = this.config.branding || {};

        if (b.has_favicon) {
            let link = document.querySelector("link[rel='icon']");
            if (link) link.href = '/branding/favicon';
        }

        if (b.has_background) {
            document.body.style.backgroundImage = "url('/branding/background')";
            document.body.style.backgroundSize = 'cover';
            document.body.style.backgroundPosition = 'center';
            document.body.style.backgroundAttachment = 'fixed';
        }

        if (b.has_custom_css) {
            const link = document.createElement('link');
            link.rel = 'stylesheet';
            link.href = '/branding/custom.css';
            document.head.appendChild(link);
        }
    },

    async openLegal(type) {
        const endpoint = type === 'impressum' ? '/legal/impressum' : '/legal/datenschutz';
        this.legalTitle = type === 'impressum' ? this.t('imprint') : this.t('privacy');
        try {
            const res = await fetch(endpoint);
            if (res.ok) {
                this.legalContent = await res.text();
                this.showLegalModal = true;
            }
        } catch (e) {
            console.warn("Failed to load legal content", e);
        }
    },

    closeLegal() {
        this.showLegalModal = false;
        this.legalContent = '';
    },

    toggleTheme() {
        this.darkMode = !this.darkMode;
        applyTheme(this.darkMode ? 'dark' : 'light');
    },

    async login() {
        const username = this.config.has_users ? this.usernameInput : null;
        const res = await API.login(username, this.passwordInput);
        if (res.success) {
            this.authenticated = true;
            this.loginError = '';
            this.userRole = res.role || 'standard';
            this.sessionUsername = res.username || null;
            this.startSession();
        } else {
            this.loginError = res.message || this.t('passwordWrong');
        }
    },

    isGuest() {
        return this.userRole === 'guest';
    },

    isAdmin() {
        return this.userRole === 'admin';
    },

    roleBadge() {
        if (this.userRole === 'admin') return this.t('roleAdmin');
        if (this.userRole === 'guest') return this.t('roleGuest');
        return null;
    },

    async logout() {
        await API.logout();
        window.location.reload();
    },

    startSession() {
        if (!this.currentUser?.name) {
            this.openSettings();
        } else {
            this.connectRtc();
        }
        this.refreshRooms();
    },

    async refreshRooms() {
        const allRooms = await API.fetchRooms();
        this.rooms = allRooms.filter(r => r.name !== LOBBY);
    },

    connectRtc() {
        rtc.setUser(this.currentUser);
        rtc.connect();
    },

    joinRoom(room) {
        if (this.currentRoom === room.name) return;

        if (room.is_locked) {
            this.roomPasswordPrompt = room.name;
            this.roomPasswordInput = '';
            return;
        }

        this.doJoinRoom(room.name);
    },

    submitRoomPassword() {
        if (!this.roomPasswordPrompt) return;
        const roomName = this.roomPasswordPrompt;
        const password = this.roomPasswordInput;
        this.roomPasswordPrompt = null;
        this.roomPasswordInput = '';
        this.doJoinRoom(roomName, password);
    },

    doJoinRoom(roomName, password) {
        if (roomName !== LOBBY) {
            rtc.startMic()
                .then(() => rtc.sendJoin(roomName, password))
                .catch(() => alert(this.t('micRequired')));
        } else {
            rtc.sendJoin(roomName);
        }
    },

    leaveRoom() {
        rtc.sendJoin(LOBBY);
    },

    toggleMute() {
        this.isMuted = rtc.toggleMute();
    },

    getLobbyUsers() {
        return this.roomState[LOBBY] || [];
    },

    getUsersInRoom(roomName) {
        return this.roomState[roomName] || [];
    },

    getPeerSetting(name) {
        if (!this.peersSettings[name]) {
            this.peersSettings[name] = { vol: 100, muted: false };
        }
        return this.peersSettings[name];
    },

    updatePeerVolume(id, name, vol) {
        this.peersSettings[name].vol = vol;
        if (!this.peersSettings[name].muted) {
            rtc.setPeerVolume(id, vol / 100);
        }
        this.savePeerSettings();
    },

    isPeerMuted(name) {
        return this.peersSettings[name]?.muted || false;
    },

    togglePeerMute(id, name) {
        const s = this.getPeerSetting(name);
        if (!s.muted) {
            rtc.setPeerVolume(id, 0);
        } else {
            rtc.setPeerVolume(id, s.vol / 100);
        }
        this.savePeerSettings();
    },

    getPeerConnectionClass(userId) {
        const state = this.peerStates[userId];
        if (!state || state === 'new' || state === 'closed') return 'conn-none';
        if (state === 'connected') return 'conn-ok';
        if (state === 'connecting') return 'conn-pending';
        return 'conn-error';
    },

    // --- Settings ---

    async openSettings() {
        this.showSettings = true;
        await this.refreshDevices();
    },

    async refreshDevices() {
        const devs = await rtc.getDevices();
        this.devices.inputs = devs.filter(d => d.kind === 'audioinput');
        this.devices.outputs = devs.filter(d => d.kind === 'audiooutput');

        if (!this.settingsForm.audioInput && this.devices.inputs.length) {
            this.settingsForm.audioInput = this.devices.inputs[0].deviceId;
        }
        if (!this.settingsForm.audioOutput && this.devices.outputs.length) {
            this.settingsForm.audioOutput = this.devices.outputs[0].deviceId;
        }

        this.restartMicPreview();
    },

    restartMicPreview() {
        rtc.startPreview(this.settingsForm.audioInput);
    },

    updateRtcSettings() {
        rtc.micThreshold = parseInt(this.settingsForm.micThreshold);
        rtc.masterVolume = parseFloat(this.settingsForm.masterVolume);
        rtc.setMicGain(parseFloat(this.settingsForm.micGain));
        rtc.echoCancellation = this.settingsForm.echoCancellation;
        rtc.noiseSuppression = this.settingsForm.noiseSuppression;
        rtc.autoGainControl = this.settingsForm.autoGainControl;
        rtc.updateAllVolumes();
    },

    toggleLoopback() {
        rtc.setLoopback(this.loopbackEnabled);
    },

    closeSettingsIfAllowed() {
        if (this.currentUser?.name) this.saveSettings();
    },

    saveSettings() {
        if (!this.settingsForm.name.trim()) return;

        this.loopbackEnabled = false;
        rtc.setLoopback(false);
        rtc.stopPreview();

        const prevEC = this.currentUser?.settings?.echoCancellation;
        const prevNS = this.currentUser?.settings?.noiseSuppression;
        const prevAGC = this.currentUser?.settings?.autoGainControl;

        this.currentUser = {
            name: this.settingsForm.name,
            audioInput: this.settingsForm.audioInput,
            audioOutput: this.settingsForm.audioOutput,
            settings: {
                micThreshold: parseInt(this.settingsForm.micThreshold),
                masterVolume: parseFloat(this.settingsForm.masterVolume),
                micGain: parseFloat(this.settingsForm.micGain),
                echoCancellation: this.settingsForm.echoCancellation,
                noiseSuppression: this.settingsForm.noiseSuppression,
                autoGainControl: this.settingsForm.autoGainControl
            }
        };

        localStorage.setItem(STORAGE_KEY_USER, JSON.stringify(this.currentUser));
        this.showSettings = false;
        rtc.setUser(this.currentUser);

        const constraintsChanged =
            prevEC !== this.currentUser.settings.echoCancellation ||
            prevNS !== this.currentUser.settings.noiseSuppression ||
            prevAGC !== this.currentUser.settings.autoGainControl;

        if (!rtc.ws) {
            this.connectRtc();
        } else {
            if (this.currentRoom === LOBBY) {
                rtc.stopMic();
            } else if (constraintsChanged) {
                rtc.stopMic();
                rtc.startMic().then(() => {
                    for (const peerId in rtc.peers) {
                        const sender = rtc.peers[peerId].pc.getSenders()
                            .find(s => s.track?.kind === 'audio');
                        if (sender && rtc.localStream) {
                            sender.replaceTrack(rtc.localStream.getAudioTracks()[0]);
                        }
                    }
                });
            } else {
                rtc.applyDeviceChange(
                    this.currentUser.audioInput,
                    this.currentUser.audioOutput
                );
            }
        }
    },

    loadUser() {
        const stored = localStorage.getItem(STORAGE_KEY_USER);
        if (stored) {
            this.currentUser = JSON.parse(stored);
            this.settingsForm.name = this.currentUser.name;
            this.settingsForm.audioInput = this.currentUser.audioInput || '';
            this.settingsForm.audioOutput = this.currentUser.audioOutput || '';
            if (this.currentUser.settings) {
                this.settingsForm.micThreshold = this.currentUser.settings.micThreshold;
                this.settingsForm.masterVolume = this.currentUser.settings.masterVolume;
                if (this.currentUser.settings.micGain !== undefined)
                    this.settingsForm.micGain = this.currentUser.settings.micGain;
                if (this.currentUser.settings.echoCancellation !== undefined)
                    this.settingsForm.echoCancellation = this.currentUser.settings.echoCancellation;
                if (this.currentUser.settings.noiseSuppression !== undefined)
                    this.settingsForm.noiseSuppression = this.currentUser.settings.noiseSuppression;
                if (this.currentUser.settings.autoGainControl !== undefined)
                    this.settingsForm.autoGainControl = this.currentUser.settings.autoGainControl;
            }
        }

        const pStore = localStorage.getItem(STORAGE_KEY_PEERS);
        if (pStore) this.peersSettings = JSON.parse(pStore);
    },

    savePeerSettings() {
        localStorage.setItem(STORAGE_KEY_PEERS, JSON.stringify(this.peersSettings));
    },

    // --- RTC Event Handler ---

    handleRtcEvent(event, data) {
        if (event === 'state-update') {
            this.roomState = data;
            if (this.currentRoom !== LOBBY) {
                const roomUsers = this.roomState[this.currentRoom] || [];
                roomUsers.forEach(u => {
                    if (this.currentUser && u.name !== this.currentUser.name) {
                        rtc.createPeer(u.id, false);
                    }
                });
            }
        } else if (event === 'you-joined') {
            this.currentRoom = data;
        } else if (event === 'gate-activity') {
            this.isGateOpen = data;
            const av = document.getElementById('avatar-local');
            if (av) {
                if (data) {
                    av.classList.add('speaking');
                } else {
                    av.classList.remove('speaking');
                }
            }
        } else if (event === 'audio-level') {
            if (data.id === 'preview') {
                const el = document.getElementById('micPreviewFill');
                if (el) el.style.width = Math.min(100, data.level) + '%';
                return;
            }

            if (data.id === 'local') {
                const el = document.getElementById('micPreviewFill');
                if (el) el.style.width = Math.min(100, data.level) + '%';
                return;
            }

            const av = document.getElementById('avatar-' + data.id);
            if (av) {
                let peerMuted = false;
                if (this.roomState[this.currentRoom]) {
                    const u = this.roomState[this.currentRoom].find(u => u.id === data.id);
                    if (u && this.peersSettings[u.name]?.muted) {
                        peerMuted = true;
                    }
                }
                if (!peerMuted && data.level > 10) {
                    av.classList.add('speaking');
                } else {
                    av.classList.remove('speaking');
                }
            }
        } else if (event === 'peer-connected') {
            if (this.roomState[this.currentRoom]) {
                const u = this.roomState[this.currentRoom].find(u => u.id === data);
                if (u) {
                    const s = this.getPeerSetting(u.name);
                    if (s.muted) {
                        rtc.setPeerVolume(data, 0);
                    } else {
                        rtc.setPeerVolume(data, s.vol / 100);
                    }
                }
            }
        } else if (event === 'peer-connection-state') {
            this.peerStates[data.id] = data.state;
        } else if (event === 'request-volume-refresh') {
            if (this.roomState[this.currentRoom]) {
                this.roomState[this.currentRoom].forEach(u => {
                    if (this.currentUser && u.name !== this.currentUser.name) {
                        const s = this.getPeerSetting(u.name);
                        if (!s.muted) {
                            rtc.setPeerVolume(u.id, s.vol / 100);
                        }
                    }
                });
            }
        } else if (event === 'connection-status') {
            this.connectionStatus = data;
        } else if (event === 'server-error') {
            console.warn("Server error:", data);
            if (data === 'Wrong room password') {
                alert(this.t('wrongRoomPassword'));
            }
        }
    }
});

document.addEventListener('alpine:init', () => {
    Alpine.data('buechseApp', buechseAppComponent);
});

const alpineScript = document.createElement('script');
alpineScript.src = '/assets/js/alpine_3.17.8.js';
document.head.appendChild(alpineScript);

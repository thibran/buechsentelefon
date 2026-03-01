export async function login(username, password) {
    const body = username ? { username, password } : { password };
    const res = await fetch('/api/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
    });
    return res.json();
}

export async function logout() {
    await fetch('/api/logout', { method: 'POST' });
}

export async function checkAuth() {
    try {
        const res = await fetch('/api/check-auth');
        if (!res.ok) return null;
        return await res.json();
    } catch {
        return null;
    }
}

export async function fetchConfig() {
    try {
        const res = await fetch('/api/config');
        if (res.ok) return await res.json();
    } catch (e) {
        console.warn("Failed to fetch config", e);
    }
    return {
        title: "Buechsentelefon",
        stun_servers: [],
        branding: {},
        legal: {},
        has_users: false
    };
}

export async function fetchRooms() {
    try {
        const res = await fetch('/api/rooms');
        if (res.ok) {
            const data = await res.json();
            return data.rooms;
        }
    } catch (e) {
        console.warn("Failed to fetch rooms", e);
    }
    return [];
}

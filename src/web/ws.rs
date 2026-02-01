use crate::config::verify_hash;
use crate::server::{AppState, User};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

const MAX_USERNAME_LEN: usize = 20;
const LOBBY: &str = "Lobby";

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "join")]
    Join {
        room: String,
        name: String,
        #[serde(default)]
        password: Option<String>,
    },

    #[serde(rename = "leave")]
    Leave,

    #[serde(rename = "offer")]
    Offer {
        target: Uuid,
        sdp: serde_json::Value,
    },

    #[serde(rename = "answer")]
    Answer {
        target: Uuid,
        sdp: serde_json::Value,
    },

    #[serde(rename = "candidate")]
    IceCandidate {
        target: Uuid,
        candidate: serde_json::Value,
    },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let my_id = Uuid::new_v4();
    let mut my_room: Option<String> = None;

    broadcast_state(&state, None).await;

    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if text.len() > 16384 {
                warn!("WebSocket message too large, ignoring");
                continue;
            }

            let client_msg: Result<ClientMessage, _> = serde_json::from_str(&text);

            match client_msg {
                Ok(ClientMessage::Join {
                    room,
                    name,
                    password,
                }) => {
                    let name = name.trim().to_string();
                    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
                        let _ = tx.send(Message::Text(
                            json!({
                                "type": "error",
                                "message": "Invalid username (1-20 characters)"
                            })
                            .to_string(),
                        ));
                        continue;
                    }

                    let config = state.config.read().await;
                    let is_lobby = room == LOBBY;
                    let room_config = config.find_room(&room);

                    if !is_lobby && room_config.is_none() {
                        let _ = tx.send(Message::Text(
                            json!({
                                "type": "error",
                                "message": "Room does not exist"
                            })
                            .to_string(),
                        ));
                        continue;
                    }

                    if let Some(rc) = room_config {
                        if let Some(ref hash) = rc.password_hash {
                            let pw = password.as_deref().unwrap_or("");
                            if !verify_hash(hash, pw) {
                                let _ = tx.send(Message::Text(
                                    json!({
                                        "type": "error",
                                        "message": "Wrong room password"
                                    })
                                    .to_string(),
                                ));
                                continue;
                            }
                        }
                    }

                    drop(config);

                    if let Some(old_room) = &my_room {
                        leave_room(&state, old_room, my_id).await;
                    }

                    let mut rooms = state.rooms.write().await;
                    let users = rooms.entry(room.clone()).or_default();

                    if users.len() >= 10 {
                        let _ = tx.send(Message::Text(
                            json!({
                                "type": "error",
                                "message": "Room is full (Max 10 users)"
                            })
                            .to_string(),
                        ));
                    } else {
                        let user = User {
                            id: my_id,
                            name: name.clone(),
                            tx: tx.clone(),
                        };
                        users.push(user);
                        my_room = Some(room.clone());
                        info!("User {} joined {}", name, room);
                    }
                    drop(rooms);

                    broadcast_state(
                        &state,
                        my_room.as_ref().map(|r| (my_id, r.clone())),
                    )
                    .await;
                }

                Ok(ClientMessage::Leave) => {
                    if let Some(room) = &my_room {
                        leave_room(&state, room, my_id).await;
                        my_room = None;
                        broadcast_state(&state, None).await;
                    }
                }

                Ok(ClientMessage::Offer { target, sdp }) => {
                    forward_msg(
                        &state,
                        &my_room,
                        target,
                        json!({
                            "type": "offer",
                            "src": my_id,
                            "sdp": sdp
                        }),
                    )
                    .await;
                }

                Ok(ClientMessage::Answer { target, sdp }) => {
                    forward_msg(
                        &state,
                        &my_room,
                        target,
                        json!({
                            "type": "answer",
                            "src": my_id,
                            "sdp": sdp
                        }),
                    )
                    .await;
                }

                Ok(ClientMessage::IceCandidate { target, candidate }) => {
                    forward_msg(
                        &state,
                        &my_room,
                        target,
                        json!({
                            "type": "candidate",
                            "src": my_id,
                            "candidate": candidate
                        }),
                    )
                    .await;
                }

                Err(e) => {
                    warn!("Invalid WS message: {}", e);
                }
            }
        }
    }

    send_task.abort();
    if let Some(room) = my_room {
        leave_room(&state, &room, my_id).await;
        broadcast_state(&state, None).await;
    }
}

async fn leave_room(state: &AppState, room_name: &str, my_id: Uuid) {
    let mut rooms = state.rooms.write().await;
    if let Some(users) = rooms.get_mut(room_name) {
        users.retain(|u| u.id != my_id);
        if users.is_empty() {
            rooms.remove(room_name);
        }
    }
}

async fn forward_msg(
    state: &AppState,
    my_room: &Option<String>,
    target_id: Uuid,
    payload: serde_json::Value,
) {
    if let Some(room_name) = my_room {
        let rooms = state.rooms.read().await;
        if let Some(users) = rooms.get(room_name) {
            if let Some(target_user) = users.iter().find(|u| u.id == target_id) {
                let _ = target_user.tx.send(Message::Text(payload.to_string()));
            }
        }
    }
}

async fn broadcast_state(state: &AppState, new_joiner: Option<(Uuid, String)>) {
    let rooms = state.rooms.read().await;

    let mut room_data = HashMap::new();
    for (room_name, users) in rooms.iter() {
        let user_list: Vec<_> = users
            .iter()
            .map(|u| json!({ "id": u.id, "name": u.name }))
            .collect();
        room_data.insert(room_name.clone(), user_list);
    }

    let state_msg = json!({
        "type": "state-update",
        "rooms": room_data
    })
    .to_string();

    for users in rooms.values() {
        for user in users {
            let _ = user.tx.send(Message::Text(state_msg.clone()));

            if let Some((joiner_id, ref joiner_room)) = new_joiner {
                if user.id == joiner_id {
                    let _ = user.tx.send(Message::Text(
                        json!({
                            "type": "you-joined",
                            "room": joiner_room
                        })
                        .to_string(),
                    ));
                } else {
                    let is_in_same_room = rooms
                        .get(joiner_room)
                        .map(|room_users| room_users.iter().any(|u| u.id == user.id))
                        .unwrap_or(false);

                    if is_in_same_room {
                        let _ = user.tx.send(Message::Text(
                            json!({
                                "type": "peer-joined",
                                "id": joiner_id
                            })
                            .to_string(),
                        ));
                    }
                }
            }
        }
    }
}

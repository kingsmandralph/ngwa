//! A room's messages, turned into plain rows the window can draw.
//!
//! The SDK's timeline items are rich and change often (edits, local echoes,
//! read markers). The window only needs a flat, owned snapshot, so each
//! change produces a fresh `Vec<TimelineRow>`.

use std::sync::Arc;

use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk_ui::timeline::{
    EventSendState, EventTimelineItem, MembershipChange, MsgLikeKind, Profile, TimelineDetails,
    TimelineItem, TimelineItemContent, TimelineItemKind, VirtualTimelineItem,
};

#[derive(Clone, Debug)]
pub enum TimelineRow {
    Message(MessageRow),
    /// "Today", "Yesterday", or a date, before that day's first message.
    Day(i64),
    /// Where you had read up to when you opened the room.
    ReadMarker,
    /// A small centred line: someone joined, renamed, and so on.
    Notice {
        text: String,
    },
    /// The very first event in the room: nothing older to load.
    Start,
}

#[derive(Clone, Debug)]
pub struct MessageRow {
    /// Stable across updates, for keeping UI state per message.
    pub key: String,
    /// Set once the server has the message; needed to reply to it.
    pub event_id: Option<String>,
    pub sender_id: String,
    pub sender_name: String,
    pub is_own: bool,
    pub body: String,
    pub kind: BodyKind,
    /// Milliseconds since the Unix epoch.
    pub timestamp: i64,
    pub reply: Option<ReplyPreview>,
    pub edited: bool,
    pub state: SendState,
    /// Why sending failed, when it did.
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BodyKind {
    Text,
    /// `/me waves`, shown as "* Ada waves".
    Emote,
    /// Bot and server notices.
    Notice,
    /// Images, files and the like: shown as a label until media lands.
    Media,
    /// Encrypted, deleted or unsupported: shown in italics.
    Placeholder,
}

#[derive(Clone, Debug)]
pub struct ReplyPreview {
    pub sender_name: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendState {
    Sent,
    Sending,
    Failed,
}

pub fn rows<'a>(items: impl IntoIterator<Item = &'a Arc<TimelineItem>>) -> Vec<TimelineRow> {
    items.into_iter().filter_map(|item| row(item)).collect()
}

fn row(item: &TimelineItem) -> Option<TimelineRow> {
    match item.kind() {
        TimelineItemKind::Virtual(VirtualTimelineItem::DateDivider(ts)) => {
            Some(TimelineRow::Day(crate::matrix::millis(*ts)))
        }
        TimelineItemKind::Virtual(VirtualTimelineItem::ReadMarker) => Some(TimelineRow::ReadMarker),
        TimelineItemKind::Virtual(VirtualTimelineItem::TimelineStart) => Some(TimelineRow::Start),
        TimelineItemKind::Event(event) => event_row(item, event),
    }
}

fn event_row(item: &TimelineItem, event: &EventTimelineItem) -> Option<TimelineRow> {
    let key = item.unique_id().0.clone();
    let sender_name = display_name(event.sender_profile(), event.sender().as_str());

    let (body, kind, reply, edited) = match event.content() {
        TimelineItemContent::MsgLike(msg) => {
            let (body, kind, edited) = msg_like_body(&msg.kind);
            let reply = msg.in_reply_to.as_ref().map(|r| match &r.event {
                TimelineDetails::Ready(embedded) => ReplyPreview {
                    sender_name: display_name(&embedded.sender_profile, embedded.sender.as_str()),
                    body: preview(&embedded.content).unwrap_or_else(|| "a message".into()),
                },
                _ => ReplyPreview {
                    sender_name: String::new(),
                    body: "a message".into(),
                },
            });
            (body, kind, reply, edited)
        }
        other => {
            let text = state_text(other, &sender_name)?;
            return Some(TimelineRow::Notice { text });
        }
    };

    let (state, error) = match event.send_state() {
        Some(EventSendState::NotSentYet { .. }) => (SendState::Sending, None),
        Some(EventSendState::SendingFailed { error, .. }) => {
            (SendState::Failed, Some(send_error_text(&error.to_string())))
        }
        _ => (SendState::Sent, None),
    };

    Some(TimelineRow::Message(MessageRow {
        key,
        event_id: event.event_id().map(ToString::to_string),
        sender_id: event.sender().to_string(),
        sender_name,
        is_own: event.is_own(),
        body,
        kind,
        timestamp: crate::matrix::millis(event.timestamp()),
        reply,
        edited,
        state,
        error,
    }))
}

/// Turn the SDK's error into something a person can act on.
pub fn send_error_text(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("m_forbidden") || lower.contains("power level") {
        "You don't have permission to post in this room.".into()
    } else if lower.contains("m_limit_exceeded") || lower.contains("too many requests") {
        "The server is rate limiting you. Try again in a moment.".into()
    } else if lower.contains("connect") || lower.contains("timed out") || lower.contains("network")
    {
        "Couldn't reach the server.".into()
    } else {
        raw.chars().take(160).collect()
    }
}

fn msg_like_body(kind: &MsgLikeKind) -> (String, BodyKind, bool) {
    match kind {
        MsgLikeKind::Message(message) => {
            let edited = message.is_edited();
            let body = message.body().to_owned();
            let (body, kind) = match message.msgtype() {
                MessageType::Emote(_) => (body, BodyKind::Emote),
                MessageType::Notice(_) | MessageType::ServerNotice(_) => (body, BodyKind::Notice),
                MessageType::Image(_) => (format!("Image: {body}"), BodyKind::Media),
                MessageType::Video(_) => (format!("Video: {body}"), BodyKind::Media),
                MessageType::Audio(_) => (format!("Audio: {body}"), BodyKind::Media),
                MessageType::File(_) => (format!("File: {body}"), BodyKind::Media),
                MessageType::Location(_) => ("Shared a location".into(), BodyKind::Media),
                _ => (body, BodyKind::Text),
            };
            (body, kind, edited)
        }
        MsgLikeKind::Sticker(sticker) => (
            format!("Sticker: {}", sticker.content().body),
            BodyKind::Media,
            false,
        ),
        MsgLikeKind::Poll(_) => ("Poll".into(), BodyKind::Media, false),
        MsgLikeKind::Redacted => ("Message deleted".into(), BodyKind::Placeholder, false),
        MsgLikeKind::UnableToDecrypt(_) => (
            "Encrypted message. Ngwa can read these from v0.2.".into(),
            BodyKind::Placeholder,
            false,
        ),
        _ => ("Unsupported message".into(), BodyKind::Placeholder, false),
    }
}

/// A one-line summary of an event, for reply quotes and room previews.
pub fn preview(content: &TimelineItemContent) -> Option<String> {
    match content {
        TimelineItemContent::MsgLike(msg) => {
            let (body, _, _) = msg_like_body(&msg.kind);
            Some(first_line(&body))
        }
        _ => None,
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    // Skip the "> quoted" fallback some clients put at the top of replies.
    let line = if line.starts_with("> ") {
        text.lines()
            .find(|l| !l.starts_with('>') && !l.trim().is_empty())
            .unwrap_or(line)
    } else {
        line
    };
    line.chars().take(140).collect()
}

/// Membership and profile changes as short sentences. Other state changes
/// (power levels, server ACLs...) are left out: they are noise in a chat.
fn state_text(content: &TimelineItemContent, sender: &str) -> Option<String> {
    match content {
        TimelineItemContent::MembershipChange(change) => {
            let who = change
                .display_name()
                .unwrap_or_else(|| change.user_id().to_string());
            let text = match change.change()? {
                MembershipChange::Joined => format!("{who} joined"),
                MembershipChange::Left => format!("{who} left"),
                MembershipChange::Invited => format!("{sender} invited {who}"),
                MembershipChange::InvitationAccepted => format!("{who} accepted an invite"),
                MembershipChange::Kicked => format!("{sender} removed {who}"),
                MembershipChange::Banned | MembershipChange::KickedAndBanned => {
                    format!("{sender} banned {who}")
                }
                MembershipChange::Unbanned => format!("{sender} unbanned {who}"),
                _ => return None,
            };
            Some(text)
        }
        TimelineItemContent::ProfileChange(change) => {
            let who = change.user_id().to_string();
            if let Some(name) = change.displayname_change() {
                return Some(match (&name.old, &name.new) {
                    (_, Some(new)) => {
                        format!("{} is now {new}", name.old.as_deref().unwrap_or(&who))
                    }
                    (Some(old), None) => format!("{old} removed their display name"),
                    (None, None) => return None,
                });
            }
            None
        }
        _ => None,
    }
}

fn display_name(profile: &TimelineDetails<Profile>, user_id: &str) -> String {
    match profile {
        TimelineDetails::Ready(Profile {
            display_name: Some(name),
            ..
        }) if !name.trim().is_empty() => name.clone(),
        // "@ada:example.org" -> "ada" until the profile loads.
        _ => user_id
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or(user_id)
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_reply_fallback() {
        assert_eq!(
            first_line("> <@a:b> hi\n> there\n\nreal reply"),
            "real reply"
        );
        assert_eq!(first_line("\n\nhello\nworld"), "hello");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn send_errors_are_readable() {
        assert_eq!(
            send_error_text("the server returned an error: [403 / M_FORBIDDEN] not allowed"),
            "You don't have permission to post in this room."
        );
        assert_eq!(send_error_text("something odd"), "something odd");
    }

    #[test]
    fn names_fall_back_to_the_user_part() {
        let pending = TimelineDetails::<Profile>::Pending;
        assert_eq!(display_name(&pending, "@ada:example.org"), "ada");
    }
}

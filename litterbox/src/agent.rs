use anyhow::Result;
use futures::Future;
use russh::keys::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use strum_macros::{Display, EnumString};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::files::SshSockFile;
use crate::{
    env::litterbox_binary_path,
    utils::{extract_stdout, trace_arguments},
};

#[derive(PartialEq, Eq, Hash, Display, Clone, Copy, EnumString)]
pub enum UserRequest {
    RequestKeys,
    AddKeys,
    RemoveKeys,
    RemoveAllKeys,
    Sign,
    Lock,
    Unlock,
}

impl From<agent::server::MessageType> for UserRequest {
    fn from(value: agent::server::MessageType) -> Self {
        use agent::server::MessageType;

        match value {
            MessageType::RequestKeys => UserRequest::RequestKeys,
            MessageType::AddKeys => UserRequest::AddKeys,
            MessageType::RemoveKeys => UserRequest::RemoveKeys,
            MessageType::RemoveAllKeys => UserRequest::RemoveAllKeys,
            MessageType::Sign => UserRequest::Sign,
            MessageType::Lock => UserRequest::Lock,
            MessageType::Unlock => UserRequest::Unlock,
        }
    }
}

#[derive(Debug, EnumString, Display)]
pub enum UserResponse {
    Approved,
    Declined,
    ApprovedForSession,
    ApprovedFor10s,
}

pub struct AgentState {
    /// When the agent is locked, users will need to approve requests
    pub locked: AtomicBool,

    /// When set, users no longer need to approve requests to list keys
    pub approved_for_session: AtomicBool,

    /// When set, signing requests are automatically approved until this instant
    pub sign_approved_until: Mutex<Option<Instant>>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            locked: AtomicBool::new(false),
            approved_for_session: AtomicBool::new(false),
            sign_approved_until: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
struct AskAgent {
    lbx_name: String,
    litterbox_path: PathBuf,
    agent_state: Arc<AgentState>,
}

impl agent::server::Agent for AskAgent {
    fn confirm(
        self,
        _: std::sync::Arc<PrivateKey>,
    ) -> Box<dyn Future<Output = (Self, bool)> + Send + Unpin> {
        todo!("Confirm private key")
    }

    async fn confirm_request(&self, msg: agent::server::MessageType) -> bool {
        let request: UserRequest = msg.into();

        if !self.agent_state.locked.load(Ordering::SeqCst) {
            log::debug!(
                "Agent not locked, request automatically approved: {}",
                request
            );
            return true;
        }

        if request == UserRequest::RequestKeys
            && self.agent_state.approved_for_session.load(Ordering::SeqCst)
        {
            log::info!("RequestKeys approved for session, not prompting.");
            return true;
        }

        if request == UserRequest::Sign {
            let approved_until = *self.agent_state.sign_approved_until.lock().await;
            if approved_until.is_some_and(|deadline| Instant::now() < deadline) {
                log::info!("Sign request approved within 10s window, not prompting.");
                return true;
            }
        }

        let mut cmd = Command::new(&self.litterbox_path);
        cmd.args([
            "confirm",
            "--request",
            &request.to_string(),
            "--lbx-name",
            &self.lbx_name,
        ]);
        trace_arguments(cmd.as_std());
        let output = cmd
            .output()
            .await
            .expect("Litterbox should return valid output to itself.");

        let stdout =
            extract_stdout(&output).expect("Litterbox should return valid output to itself.");

        // STDOUT might contain a newline
        let response = stdout.trim();

        if let Ok(response) = response.parse() {
            match response {
                UserResponse::Approved => true,
                UserResponse::Declined => false,
                UserResponse::ApprovedForSession => {
                    self.agent_state
                        .approved_for_session
                        .store(true, Ordering::SeqCst);

                    true
                }
                UserResponse::ApprovedFor10s => {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    *self.agent_state.sign_approved_until.lock().await = Some(deadline);

                    true
                }
            }
        } else {
            log::error!("Unexpected confirmation response '{response}'");

            false
        }
    }
}

pub async fn start_ssh_agent(lbx_name: &str, agent_state: Arc<AgentState>) -> Result<PathBuf> {
    let litterbox_path = litterbox_binary_path();

    let ssh_sock = SshSockFile::new(lbx_name, false)?;
    let agent_path = ssh_sock.path().to_owned();

    let ssh_sock_path = ssh_sock.path();
    log::debug!("Binding SSH socket: {:#?}", ssh_sock_path);
    let listener =
        tokio::net::UnixListener::bind(ssh_sock_path).expect("SSH socket should be bindable");

    let lbx_name = lbx_name.to_string();
    tokio::spawn(async move {
        log::debug!("Starting SSH agent server task");

        // We need to keep the socket object alive to prevent the file from getting deleted
        let _ssh_sock = ssh_sock;

        russh::keys::agent::server::serve(
            tokio_stream::wrappers::UnixListenerStream::new(listener),
            AskAgent {
                lbx_name,
                litterbox_path,
                agent_state,
            },
        )
        .await
    });

    Ok(agent_path)
}

//! CT-CONNECT-RETRY-001: the request body is not available to this module.
//! This opt-in boundary ends before PoolClient::try_send_request is invoked.

use super::{Error, ErrorKind};
use crate::common::Exec;
use futures_util::future::{AbortHandle, Abortable};
use std::{error::Error as StdError, future::Future, io, time::Duration};

pub(super) type Classifier = fn(&(dyn StdError + 'static)) -> bool;

#[derive(Clone, Copy, Debug)]
pub(super) struct Policy {
    pub classify: Classifier,
    pub timeout: Option<Duration>,
}

pub(super) async fn connect<T, F, Fut>(policy: Policy, mut attempt: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let run = async {
        for index in 0..3 {
            match attempt().await {
                Ok(connection) => return Ok(connection),
                Err(error) => {
                    if index == 2 || !(policy.classify)(&error) {
                        return Err(error);
                    }
                    tokio::time::sleep(Duration::from_millis(200 << index)).await;
                }
            }
        }
        unreachable!("fixed connection setup budget")
    };
    // One connect deadline surrounds all attempts and waits. The caller's
    // request deadline/cancellation drops this entire future, never a detached
    // connection retry task. biased preserves deadline precedence if both wake.
    if let Some(timeout) = policy.timeout {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(timeout) => Err(Error {
                kind: ErrorKind::Connect,
                source: Some(Box::new(io::Error::new(io::ErrorKind::TimedOut, "connection setup deadline elapsed"))),
                connect_info: None,
            }),
            result = run => result,
        }
    } else {
        run.await
    }
}

/// During HTTP/2 ready/handshake, a spawned dispatcher owns the partial socket.
/// Dropping the setup future must also drop that dispatcher and socket. After
/// setup succeeds, normal sender/response ownership takes over.
pub(super) struct SetupDispatch(Option<AbortHandle>);

impl SetupDispatch {
    pub fn spawn(
        executor: &Exec,
        future: impl Future<Output = ()> + Send + 'static,
        owned: bool,
    ) -> Self {
        if owned {
            let (abort, registration) = AbortHandle::new_pair();
            executor.execute(async move {
                let _ = Abortable::new(future, registration).await;
            });
            Self(Some(abort))
        } else {
            executor.execute(future);
            Self(None)
        }
    }

    pub fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for SetupDispatch {
    fn drop(&mut self) {
        if let Some(abort) = self.0.take() {
            abort.abort();
        }
    }
}

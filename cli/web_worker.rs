// Copyright 2018-2020 the Deno authors. All rights reserved. MIT license.
use crate::ops;
use crate::state::State;
use crate::worker::Worker;
use deno_core;
use deno_core::ErrBox;
use deno_core::StartupData;
use futures::future::FutureExt;
use std::future::Future;
use std::ops::Deref;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

/// This worker is implementation of `Worker` Web API
///
/// At the moment this type of worker supports only
/// communication with parent and creating new workers.
///
/// Each `WebWorker` is either a child of `MainWorker` or other
/// `WebWorker`.
pub struct WebWorker(Worker);

impl WebWorker {
  pub fn new(name: String, startup_data: StartupData, state: State) -> Self {
    let state_ = state.clone();
    let mut worker = Worker::new(name, startup_data, state_);
    {
      let isolate = &mut worker.isolate;
      ops::runtime::init(isolate, &state);
      ops::web_worker::init(isolate, &state);
      ops::worker_host::init(isolate, &state);
      ops::errors::init(isolate, &state);
      ops::timers::init(isolate, &state);
      ops::fetch::init(isolate, &state);
    }

    Self(worker)
  }
}

impl Deref for WebWorker {
  type Target = Worker;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for WebWorker {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

impl Future for WebWorker {
  type Output = Result<(), ErrBox>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
    let inner = self.get_mut();
    inner.0.poll_unpin(cx)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::startup_data;
  use crate::state::State;
  use crate::tokio_util;

  fn create_test_worker() -> WebWorker {
    let state = State::mock("./hello.js");
    let mut worker = WebWorker::new(
      "TEST".to_string(),
      startup_data::deno_isolate_init(),
      state,
    );
    worker.execute("bootstrapWorkerRuntime(\"TEST\")").unwrap();
    worker.execute("runWorkerMessageLoop()").unwrap();
    worker
  }

  #[test]
  fn test_worker_messages() {
    let mut worker = create_test_worker();
    let source = r#"
        onmessage = function(e) {
          console.log("msg from main script", e.data);
          if (e.data == "exit") {
            delete self.onmessage;
            return;
          } else {
            console.assert(e.data === "hi");
          }
          postMessage([1, 2, 3]);
          console.log("after postMessage");
        }
        "#;
    worker.execute(source).unwrap();

    let handle = worker.thread_safe_handle();
    let _ = tokio_util::spawn_thread(move || {
      tokio_util::run_basic(async move {
        let msg = json!("hi").to_string().into_boxed_str().into_boxed_bytes();
        let r = handle.post_message(msg.clone()).await;
        assert!(r.is_ok());

        let maybe_msg = handle.get_message().await;
        assert!(maybe_msg.is_some());

        let r = handle.post_message(msg.clone()).await;
        assert!(r.is_ok());

        let maybe_msg = handle.get_message().await;
        assert!(maybe_msg.is_some());
        assert_eq!(*maybe_msg.unwrap(), *b"[1,2,3]");

        let msg = json!("exit")
          .to_string()
          .into_boxed_str()
          .into_boxed_bytes();
        let r = handle.post_message(msg).await;
        assert!(r.is_ok());
      })
    });

    let r = tokio_util::run_basic(worker);
    assert!(r.is_ok())
  }

  #[test]
  fn removed_from_resource_table_on_close() {
    let mut worker = create_test_worker();
    let handle = worker.thread_safe_handle();

    worker
      .execute("onmessage = () => { delete self.onmessage; }")
      .unwrap();

    let worker_post_message_fut = tokio_util::spawn_thread(move || {
      let msg = json!("hi").to_string().into_boxed_str().into_boxed_bytes();
      let r = futures::executor::block_on(handle.post_message(msg));
      assert!(r.is_ok());
    });

    tokio_util::run_basic(async move {
      worker_post_message_fut.await;
      let r = worker.await;
      assert!(r.is_ok());
    });
  }
}

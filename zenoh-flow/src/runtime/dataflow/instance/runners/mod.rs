//
// Copyright (c) 2017, 2021 ADLINK Technology Inc.
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ADLINK zenoh team, <zenoh@adlink-labs.tech>
//

pub mod connector;
pub mod logging;
pub mod operator;
pub mod replay;
pub mod sink;
pub mod source;

use self::logging::ZenohLogger;
use crate::async_std::prelude::*;
use crate::async_std::sync::Arc;
use crate::async_std::task::JoinHandle;

use crate::runtime::dataflow::instance::link;
use crate::runtime::dataflow::instance::link::{LinkReceiver, LinkSender};
use crate::runtime::message::Message;
use crate::runtime::InstanceContext;
use crate::types::{NodeId, ZFResult};
use crate::{PortId, PortType, ZFError};
use async_trait::async_trait;
use futures_lite::future::FutureExt;
use std::collections::HashMap;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};
use zenoh_util::sync::Signal;

#[derive(Clone, Debug, PartialEq)]
pub enum RunnerKind {
    Source,
    Operator,
    Sink,
    Connector,
}

pub struct RunnerManager {
    stopper: Signal,
    handler: JoinHandle<ZFResult<()>>,
    runner: Arc<dyn Runner>,
    ctx: InstanceContext,
    logger: Option<ZenohLogger>,
    logger_stopper: Option<Signal>,
}

impl RunnerManager {
    pub async fn try_new(
        stopper: Signal,
        handler: JoinHandle<ZFResult<()>>,
        runner: Arc<dyn Runner>,
        ctx: InstanceContext,
    ) -> ZFResult<Self> {
        if runner.get_kind() == RunnerKind::Source {
            let node_id = runner.get_id();
            let mut outputs: Vec<(PortId, PortType)> = runner
                .get_outputs()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let (output_id, _output_type) = outputs
                .pop()
                .ok_or_else(|| ZFError::NodeNotFound(node_id.clone()))?;

            // creating zenoh resource name
            let z_resource_name = format!(
                "/zf/record/{}/{}/{}/{}",
                &ctx.flow_id, &ctx.instance_id, node_id, output_id
            );

            let recorder_id: NodeId = format!(
                "logger-{}-{}-{}-{}",
                ctx.flow_id, ctx.instance_id, node_id, output_id
            )
            .into();

            let (tx, rx) = link::<Message>(None, output_id.clone(), output_id.clone());

            let logger = ZenohLogger::try_new(
                recorder_id,
                ctx.clone(),
                z_resource_name,
                node_id,
                output_id,
                rx,
            )?;

            runner.add_output(tx).await?;

            let logger_stopper = logger.start();

            return Ok(Self {
                stopper,
                handler,
                runner,
                ctx,
                logger: Some(logger),
                logger_stopper: Some(logger_stopper),
            });
        }

        Ok(Self {
            stopper,
            handler,
            runner,
            ctx,
            logger: None,
            logger_stopper: None,
        })
    }

    pub async fn kill(&self) -> ZFResult<()> {
        if let Some(ls) = &self.logger_stopper {
            ls.trigger()
        }
        self.stopper.trigger();
        Ok(())
    }

    pub fn get_handler(&self) -> &JoinHandle<ZFResult<()>> {
        &self.handler
    }

    pub async fn start_recording(&self) -> ZFResult<String> {
        match &self.logger {
            Some(logger) => logger.start_recording().await,
            _ => Ok(String::from("")),
        }
    }

    pub async fn stop_recording(&self) -> ZFResult<String> {
        match &self.logger {
            Some(logger) => logger.stop_recording().await,
            _ => Ok(String::from("")),
        }
    }

    pub fn get_context(&self) -> &InstanceContext {
        &self.ctx
    }
}

impl Deref for RunnerManager {
    type Target = Arc<dyn Runner>;

    fn deref(&self) -> &Self::Target {
        &self.runner
    }
}

impl Future for RunnerManager {
    type Output = ZFResult<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        self.handler.poll(ctx)
    }
}

pub enum RunAction {
    RestartRun(Option<ZFError>),
    Stop,
}

#[async_trait]
pub trait Runner: Send + Sync {
    async fn run(&self) -> ZFResult<()>;
    async fn add_input(&self, input: LinkReceiver<Message>) -> ZFResult<()>;

    async fn add_output(&self, output: LinkSender<Message>) -> ZFResult<()>;

    async fn clean(&self) -> ZFResult<()>;

    fn get_kind(&self) -> RunnerKind;

    fn get_id(&self) -> NodeId;

    fn get_inputs(&self) -> HashMap<PortId, PortType>;

    fn get_outputs(&self) -> HashMap<PortId, PortType>;

    async fn get_outputs_links(&self) -> HashMap<PortId, Vec<LinkSender<Message>>>;

    async fn get_input_links(&self) -> HashMap<PortId, LinkReceiver<Message>>;
}

#[derive(Clone)]
pub struct NodeRunner {
    inner: Arc<dyn Runner>,
    ctx: InstanceContext,
}

impl NodeRunner {
    pub fn new(inner: Arc<dyn Runner>, ctx: InstanceContext) -> Self {
        Self { inner, ctx }
    }

    fn run_stoppable(&self, signal: Signal) -> ZFResult<()> {
        loop {
            async fn run(runner: &NodeRunner) -> RunAction {
                match runner.run().await {
                    Ok(_) => RunAction::Stop,
                    Err(e) => RunAction::RestartRun(Some(e)),
                }
            }

            async fn stop(signal: Signal) -> RunAction {
                signal.wait().await;
                RunAction::Stop
            }
            let cloned_signal = signal.clone();
            let result = async_std::task::block_on(async move {
                match stop(cloned_signal).race(run(self)).await {
                    RunAction::RestartRun(e) => {
                        log::error!(
                            "[Node: {}] The run loop exited with {:?}, restarting…",
                            self.get_id(),
                            e
                        );
                        Err(e)
                    }
                    RunAction::Stop => {
                        log::trace!(
                            "[Node: {}] Received kill command, killing runner",
                            self.get_id()
                        );
                        Ok(())
                    }
                }
            });

            if result.is_ok() {
                return Ok(());
            }
        }
    }
    pub async fn start(&self) -> ZFResult<RunnerManager> {
        let signal = Signal::new();
        let cloned_self = self.clone();
        let cloned_signal = signal.clone();

        let h = async_std::task::spawn_blocking(move || cloned_self.run_stoppable(cloned_signal));

        RunnerManager::try_new(signal, h, self.inner.clone(), self.ctx.clone()).await
    }
}

impl Deref for NodeRunner {
    type Target = Arc<dyn Runner>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[macro_export]
macro_rules! run_input_rules {
    ($node: expr, $tokens : expr, $links : expr, $state: expr, $context: expr) => {
        while !$links.is_empty() {
            match future::select_all($links).await {
                // this could be "slow" as suggested by LC
                (Ok((id, message)), _i, remaining) => {
                    match message.as_ref() {
                        Message::Data(_) => {
                            $tokens.insert(id, Token::from(message));

                            match $node.input_rule($context, $state, &mut $tokens) {
                                Ok(true) => {
                                    // we can run
                                    log::debug!("IR: OK");
                                    $links = vec![]; // this makes the while loop to end
                                }
                                Ok(false) => {
                                    //we cannot run, we should update the list of futures
                                    log::debug!("IR: Not OK");
                                    $links = remaining;
                                }
                                Err(_) => {
                                    // we got an error on the input rules, we should recover/update list of futures
                                    log::debug!("IR: received an error");
                                    $links = remaining;
                                }
                            }
                        }
                        Message::Control(_) => {
                            //control message receiver, we should handle it
                            $links = remaining;
                        }
                    };
                }
                (Err(e), i, remaining) => {
                    log::debug!("Link index {:?} has got error {:?}", i, e);
                    $links = remaining;
                }
            }
        };
        drop($links);
    };
}

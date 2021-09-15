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

use async_std::sync::Arc;
use async_trait::async_trait;
use std::collections::HashMap;
use zenoh_flow::{
    default_input_rule, downcast, get_input, types::ZFResult, zenoh_flow_derive::ZFState,
    StateTrait, ZFComponent, ZFComponentInputRule, ZFError, ZFSinkTrait,
};
use zenoh_flow_examples::ZFBytes;

use opencv::{highgui, prelude::*};

static INPUT: &str = "Frame";

#[derive(Debug)]
struct VideoSink;

#[derive(ZFState, Clone, Debug)]
struct VideoState {
    pub window_name: String,
}

impl VideoState {
    pub fn new() -> Self {
        let window_name = &"Video-Sink".to_string();
        highgui::named_window(window_name, 1).unwrap();
        Self {
            window_name: window_name.to_string(),
        }
    }
}

impl ZFComponentInputRule for VideoSink {
    fn input_rule(
        &self,
        _context: &mut zenoh_flow::Context,
        state: &mut Box<dyn zenoh_flow::StateTrait>,
        tokens: &mut HashMap<zenoh_flow::PortId, zenoh_flow::Token>,
    ) -> zenoh_flow::ZFResult<bool> {
        default_input_rule(state, tokens)
    }
}

impl ZFComponent for VideoSink {
    fn initialize(
        &self,
        _configuration: &Option<HashMap<String, String>>,
    ) -> Box<dyn zenoh_flow::StateTrait> {
        Box::new(VideoState::new())
    }

    fn clean(&self, state: &mut Box<dyn StateTrait>) -> ZFResult<()> {
        let state = downcast!(VideoState, state).ok_or(ZFError::MissingState)?;
        highgui::destroy_window(&state.window_name).unwrap();
        Ok(())
    }
}

#[async_trait]
impl ZFSinkTrait for VideoSink {
    async fn run(
        &self,
        _context: &mut zenoh_flow::Context,
        dyn_state: &mut Box<dyn zenoh_flow::StateTrait>,
        inputs: &mut HashMap<zenoh_flow::PortId, zenoh_flow::runtime::message::ZFDataMessage>,
    ) -> zenoh_flow::ZFResult<()> {
        // Downcasting to right type
        let state = downcast!(VideoState, dyn_state).unwrap();

        let (_, data) = get_input!(ZFBytes, String::from(INPUT), inputs).unwrap();

        let decoded = opencv::imgcodecs::imdecode(
            &opencv::types::VectorOfu8::from_iter(data.0),
            opencv::imgcodecs::IMREAD_COLOR,
        )
        .unwrap();

        if decoded.size().unwrap().width > 0 {
            highgui::imshow(&state.window_name, &decoded).unwrap();
        }

        highgui::wait_key(10).unwrap();
        Ok(())
    }
}

// Also generated by macro
zenoh_flow::export_sink!(register);

fn register() -> ZFResult<Arc<dyn ZFSinkTrait>> {
    Ok(Arc::new(VideoSink) as Arc<dyn ZFSinkTrait>)
}

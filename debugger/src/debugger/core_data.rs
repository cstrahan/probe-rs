use super::session_data;
use crate::{
    debug_adapter::{dap_adapter::DebugAdapter, protocol::ProtocolAdapter},
    debugger::debug_rtt,
    peripherals::svd_variables::SvdCache,
    DebuggerError,
};
use anyhow::Result;
use probe_rs::{debug::debug_info::DebugInfo, Core};
use probe_rs_cli_util::rtt::{self, ChannelMode, DataFormat};

/// [CoreData] is used to cache data needed by the debugger, on a per-core basis.
pub struct CoreData {
    pub(crate) core_index: usize,
    pub(crate) target_name: String,
    pub(crate) debug_info: DebugInfo,
    pub(crate) core_peripherals: Option<SvdCache>,
    pub(crate) stack_frames: Vec<probe_rs::debug::stack_frame::StackFrame>,
    pub(crate) breakpoints: Vec<session_data::ActiveBreakpoint>,
    pub(crate) rtt_connection: Option<debug_rtt::RttConnection>,
}

/// [CoreHandle] provides handles to various data structures required to debug a single instance of a core. The actual state is stored in [SessionData].
///
/// Usage: To get access to this structure please use the [SessionData::attach_core] method. Please keep access/locks to this to a minumum duration.
pub struct CoreHandle<'p> {
    pub(crate) core: Core<'p>,
    pub(crate) core_data: &'p mut CoreData,
}

impl<'p> CoreHandle<'p> {
    /// Search available [StackFrame]'s for the given `id`
    pub(crate) fn get_stackframe(
        &'p self,
        id: i64,
    ) -> Option<&'p probe_rs::debug::stack_frame::StackFrame> {
        self.core_data
            .stack_frames
            .iter()
            .find(|stack_frame| stack_frame.id == id)
    }

    /// Confirm RTT initialization on the target, and use the RTT channel configurations to initialize the output windows on the DAP Client.
    pub fn attach_to_rtt<P: ProtocolAdapter>(
        &mut self,
        debug_adapter: &mut DebugAdapter<P>,
        target_memory_map: &[probe_rs::config::MemoryRegion],
        program_binary: &std::path::Path,
        rtt_config: &rtt::RttConfig,
    ) -> Result<()> {
        let mut debugger_rtt_channels: Vec<debug_rtt::DebuggerRttChannel> = vec![];
        match rtt::attach_to_rtt(
            &mut self.core,
            target_memory_map,
            program_binary,
            rtt_config,
        ) {
            Ok(target_rtt) => {
                for any_channel in target_rtt.active_channels.iter() {
                    if let Some(up_channel) = &any_channel.up_channel {
                        if any_channel.data_format == DataFormat::Defmt {
                            // For defmt, we set the channel to be blocking when full.
                            up_channel.set_mode(&mut self.core, ChannelMode::BlockIfFull)?;
                        }
                        debugger_rtt_channels.push(debug_rtt::DebuggerRttChannel {
                            channel_number: up_channel.number(),
                            // This value will eventually be set to true by a VSCode client request "rttWindowOpened"
                            has_client_window: false,
                        });
                        debug_adapter.rtt_window(
                            up_channel.number(),
                            any_channel.channel_name.clone(),
                            any_channel.data_format,
                        );
                    }
                }
                self.core_data.rtt_connection = Some(debug_rtt::RttConnection {
                    target_rtt,
                    debugger_rtt_channels,
                });
            }
            Err(_error) => {
                log::warn!("Failed to initalize RTT. Will try again on the next request... ");
            }
        };
        Ok(())
    }

    /// Set a single breakpoint in target configuration as well as [`CoreHandle::breakpoints`]
    pub(crate) fn set_breakpoint(
        &mut self,
        address: u64,
        breakpoint_type: session_data::BreakpointType,
    ) -> Result<(), DebuggerError> {
        self.core
            .set_hw_breakpoint(address)
            .map_err(DebuggerError::ProbeRs)?;
        self.core_data
            .breakpoints
            .push(session_data::ActiveBreakpoint {
                breakpoint_type,
                breakpoint_address: address,
            });
        Ok(())
    }

    /// Clear a single breakpoint from target configuration as well as [`CoreHandle::breakpoints`]
    pub(crate) fn clear_breakpoint(&mut self, address: u64) -> Result<()> {
        self.core
            .clear_hw_breakpoint(address)
            .map_err(DebuggerError::ProbeRs)?;
        let mut breakpoint_position: Option<usize> = None;
        for (position, active_breakpoint) in self.core_data.breakpoints.iter().enumerate() {
            if active_breakpoint.breakpoint_address == address {
                breakpoint_position = Some(position);
                break;
            }
        }
        if let Some(breakpoint_position) = breakpoint_position {
            self.core_data
                .breakpoints
                .remove(breakpoint_position as usize);
        }
        Ok(())
    }

    /// Clear all breakpoints of a specified [`BreakpointType`]. Affects target configuration as well as [`CoreHandle::breakpoints`]
    pub(crate) fn clear_breakpoints(
        &mut self,
        breakpoint_type: session_data::BreakpointType,
    ) -> Result<()> {
        let target_breakpoints = self
            .core_data
            .breakpoints
            .iter()
            .filter(|breakpoint| breakpoint.breakpoint_type == breakpoint_type)
            .map(|breakpoint| breakpoint.breakpoint_address)
            .collect::<Vec<u64>>();
        for breakpoint in target_breakpoints {
            self.clear_breakpoint(breakpoint).ok();
        }
        Ok(())
    }
}

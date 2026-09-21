use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::FWPM_LAYER_ALE_AUTH_CONNECT_V4;
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::FWPM_LAYER_ALE_AUTH_CONNECT_V6;
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::{FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V4;
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V6;
use windows_sys::Win32::Networking::WinSock::IPPROTO_ICMP;
use windows_sys::Win32::Networking::WinSock::IPPROTO_ICMPV6;
use windows_sys::core::GUID;

#[derive(Clone, Copy)]
pub(super) enum ConditionSpec {
    User,
    Protocol(u8),
    RemotePort(u16),
}

#[derive(Clone, Copy)]
pub(super) struct FilterSpec {
    pub(super) key: GUID,
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) layer_key: GUID,
    pub(super) conditions: &'static [ConditionSpec],
}

pub(super) const FILTER_SPECS: &[FilterSpec] = &[
    FilterSpec {
        key: GUID::from_u128(0x8c133d02_4a82_4bfe_828d_64327281af88),
        name: "dsh_native_wfp_offline_bind_v4",
        description: "Deny offline-account IPv4 socket resource allocation before connect authorization",
        layer_key: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V4,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0x2fc9af68_c08c_43e2_807d_733d9b27b73e),
        name: "dsh_native_wfp_offline_bind_v6",
        description: "Deny offline-account IPv6 socket resource allocation before connect authorization",
        layer_key: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V6,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0xac973fd4_ef54_4bce_a03b_7b06c80540d5),
        name: "dsh_native_wfp_offline_accept_v4",
        description: "Block offline sandbox incoming IPv4 connections including loopback",
        layer_key: FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0x39eac4d3_0615_4465_abb8_5b683865bc46),
        name: "dsh_native_wfp_offline_accept_v6",
        description: "Block offline sandbox incoming IPv6 connections including loopback",
        layer_key: FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0x9c08b767_650c_530e_a5c5_71c54e9b5021),
        name: "dsh_native_wfp_offline_connect_v4",
        description: "Block all offline sandbox outbound IPv4 connections including loopback",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0x210a389a_5bc1_53b9_a1c9_ef9831387a52),
        name: "dsh_native_wfp_offline_connect_v6",
        description: "Block all offline sandbox outbound IPv6 connections including loopback",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[ConditionSpec::User],
    },
    FilterSpec {
        key: GUID::from_u128(0xe169035a7d305f74861051d2e31b24e3),
        name: "dsh_native_wfp_icmp_connect_v4",
        description: "Block sandbox-account ICMP connect v4",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[
            ConditionSpec::User,
            ConditionSpec::Protocol(IPPROTO_ICMP as u8),
        ],
    },
    FilterSpec {
        key: GUID::from_u128(0xea1c68a582915169b7fa202f8cd35265),
        name: "dsh_native_wfp_icmp_connect_v6",
        description: "Block sandbox-account ICMP connect v6",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[
            ConditionSpec::User,
            ConditionSpec::Protocol(IPPROTO_ICMPV6 as u8),
        ],
    },
    FilterSpec {
        key: GUID::from_u128(0x8c43cc733e2b50a38fac25238a4da91e),
        name: "dsh_native_wfp_icmp_assign_v4",
        description: "Block sandbox-account ICMP resource assignment v4",
        layer_key: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V4,
        conditions: &[
            ConditionSpec::User,
            ConditionSpec::Protocol(IPPROTO_ICMP as u8),
        ],
    },
    FilterSpec {
        key: GUID::from_u128(0x1e50486ea07252b293edf140deb6121c),
        name: "dsh_native_wfp_icmp_assign_v6",
        description: "Block sandbox-account ICMP resource assignment v6",
        layer_key: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V6,
        conditions: &[
            ConditionSpec::User,
            ConditionSpec::Protocol(IPPROTO_ICMPV6 as u8),
        ],
    },
    // NAME_RESOLUTION_CACHE filters are intentionally omitted because ordinary
    // static filter shapes returned FWP_E_OUT_OF_BOUNDS during validation.
    FilterSpec {
        key: GUID::from_u128(0x74cb8fd4650f58f5a4410bc8386fbe02),
        name: "dsh_native_wfp_dns_53_v4",
        description: "Block sandbox-account DNS TCP or UDP port 53 v4",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(53)],
    },
    FilterSpec {
        key: GUID::from_u128(0xcbffd5ce7702513cb2a12c05fffee7f4),
        name: "dsh_native_wfp_dns_53_v6",
        description: "Block sandbox-account DNS TCP or UDP port 53 v6",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(53)],
    },
    FilterSpec {
        key: GUID::from_u128(0x18f94eba97b95200aa4acf4502196111),
        name: "dsh_native_wfp_dns_853_v4",
        description: "Block sandbox-account DNS-over-TLS port 853 v4",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(853)],
    },
    FilterSpec {
        key: GUID::from_u128(0xbdf5f1d29edc532e8f7afd44717fe0d7),
        name: "dsh_native_wfp_dns_853_v6",
        description: "Block sandbox-account DNS-over-TLS port 853 v6",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(853)],
    },
    FilterSpec {
        key: GUID::from_u128(0x3482a05fdee55c06ad5f32b881a66281),
        name: "dsh_native_wfp_smb_445_v4",
        description: "Block sandbox-account SMB port 445 v4",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(445)],
    },
    FilterSpec {
        key: GUID::from_u128(0xdcfce770c03d5bcca39cb85410690c5f),
        name: "dsh_native_wfp_smb_445_v6",
        description: "Block sandbox-account SMB port 445 v6",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(445)],
    },
    FilterSpec {
        key: GUID::from_u128(0x9c329de1b5cc5f1c87975ee3f74b493c),
        name: "dsh_native_wfp_smb_139_v4",
        description: "Block sandbox-account SMB port 139 v4",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(139)],
    },
    FilterSpec {
        key: GUID::from_u128(0xa0f662e0f6f95273a9477eae6dc58200),
        name: "dsh_native_wfp_smb_139_v6",
        description: "Block sandbox-account SMB port 139 v6",
        layer_key: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        conditions: &[ConditionSpec::User, ConditionSpec::RemotePort(139)],
    },
];

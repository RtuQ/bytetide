//! 串口枚举与展示（`list` 子命令）。
//! Linux 无 udev 元数据（musl 静态编译关掉 feature）时，串口全部报 "unknown"：
//! 用 /dev/serial/by-id 符号链接兜底，把 by-id 文件名补进描述列。

use bytetide_core::serial::port::PortInfo;

/// 合成 list 输出行（纯函数）：`名称  类型  厂商/产品  描述`，列间两空格、按最长列对齐。
/// by_id = (by-id 文件名, 指向的设备路径)；行缺 USB 元数据（vendor/product 皆空）时
/// 用匹配的 by-id 文件名补描述，否则描述列显示序列号（sn:xxx）或留空。
pub fn format_port_rows(ports: &[PortInfo], by_id: &[(String, String)]) -> Vec<String> {
    // 厂商/产品列
    let vps: Vec<String> = ports
        .iter()
        .map(|p| match (&p.vendor, &p.product) {
            (Some(v), Some(pr)) => format!("{v} / {pr}"),
            (Some(v), None) => v.clone(),
            (None, Some(pr)) => pr.clone(),
            (None, None) => "-".to_string(),
        })
        .collect();
    // 描述列：无 USB 元数据 -> by-id 文件名；有元数据 -> 序列号
    let descs: Vec<String> = ports
        .iter()
        .map(|p| {
            if p.vendor.is_none() && p.product.is_none() {
                if let Some((f, _)) = by_id.iter().find(|(_, dev)| dev == &p.name) {
                    return f.clone();
                }
            }
            p.serial
                .as_deref()
                .map(|s| format!("sn:{s}"))
                .unwrap_or_default()
        })
        .collect();
    let name_w = ports.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let type_w = ports.iter().map(|p| p.port_type.len()).max().unwrap_or(0);
    let vp_w = vps.iter().map(|s| s.len()).max().unwrap_or(0);
    ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                "{:<nw$}  {:<tw$}  {:<vw$}  {}",
                p.name,
                p.port_type,
                vps[i],
                descs[i],
                nw = name_w,
                tw = type_w,
                vw = vp_w,
            )
            .trim_end()
            .to_string()
        })
        .collect()
}

/// 扫描 /dev/serial/by-id 符号链接（仅 Linux）：返回 (by-id 文件名, 解析后的设备路径)。
/// 读取失败（目录不存在等）静默返回空——描述兜底是尽力而为。
pub fn scan_by_id() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        let mut out: Vec<(String, String)> = std::fs::read_dir("/dev/serial/by-id")
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        // 相对 symlink：canonicalize 解析到真实设备路径
                        let dev = std::fs::canonicalize(e.path()).ok()?;
                        Some((name, dev.to_string_lossy().into_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(
        name: &str,
        ty: &str,
        vendor: Option<&str>,
        product: Option<&str>,
        serial: Option<&str>,
    ) -> PortInfo {
        PortInfo {
            name: name.into(),
            port_type: ty.into(),
            vendor: vendor.map(Into::into),
            product: product.map(Into::into),
            serial: serial.map(Into::into),
        }
    }

    #[test]
    fn usb_row_full_metadata() {
        let rows = format_port_rows(
            &[p("COM3", "usb", Some("FTDI"), Some("FT232R"), Some("A502"))],
            &[],
        );
        assert_eq!(rows, vec!["COM3  usb  FTDI / FT232R  sn:A502".to_string()]);
    }

    #[test]
    fn pci_row_without_metadata() {
        let rows = format_port_rows(&[p("/dev/ttyS4", "pci", None, None, None)], &[]);
        // 描述列空：行尾去空白
        assert_eq!(rows, vec!["/dev/ttyS4  pci  -".to_string()]);
    }

    #[test]
    fn by_id_enrichment_and_alignment() {
        let ports = vec![
            p("COM3", "usb", Some("FTDI"), Some("FT232R"), Some("A502")),
            p("/dev/ttyUSB0", "unknown", None, None, None),
        ];
        let by_id = vec![(
            "usb-FTDI_FT232R_A502-if00-port0".to_string(),
            "/dev/ttyUSB0".to_string(),
        )];
        let rows = format_port_rows(&ports, &by_id);
        // 列宽：名称 12 / 类型 7 / 厂商产品 13 —— 对齐后逐字节断言
        assert_eq!(rows[0], "COM3          usb      FTDI / FT232R  sn:A502");
        assert_eq!(
            rows[1],
            "/dev/ttyUSB0  unknown  -              usb-FTDI_FT232R_A502-if00-port0"
        );
        // by_id 不匹配（设备路径对不上）时不补描述
        let rows2 = format_port_rows(&ports, &[("usb-other-if00".into(), "/dev/ttyACM0".into())]);
        assert_eq!(rows2[1], "/dev/ttyUSB0  unknown  -");
    }
}

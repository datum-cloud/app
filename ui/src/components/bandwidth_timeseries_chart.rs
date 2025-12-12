use chrono::{DateTime, Local};
use dioxus::prelude::*;

use crate::util::humanize_bytes;

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub ts: DateTime<Local>,
    pub send: u64,
    pub recv: u64,
}

impl std::default::Default for ChartData {
    fn default() -> Self {
        Self {
            ts: Local::now(),
            send: 0,
            recv: 0,
        }
    }
}

#[derive(PartialEq, Clone, Props)]
pub struct BwTsChartProps {
    pub data: Vec<ChartData>,
}

// Generate SVG path for a line
fn generate_path(
    data: &[ChartData],
    get_value: fn(&ChartData) -> u64,
    width: f64,
    height: f64,
    max_value: f64,
) -> String {
    if data.is_empty() {
        return String::new();
    }

    let points: Vec<String> = data
        .iter()
        .enumerate()
        .map(|(i, point)| {
            let x = (i as f64 / (data.len() - 1).max(1) as f64) * width;
            let value = get_value(point) as f64;
            let y = if max_value > 0.0 {
                height - (value / max_value * height)
            } else {
                height
            };
            format!("{},{}", x, y)
        })
        .collect();

    if points.is_empty() {
        return String::new();
    }

    format!("M {}", points.join(" L "))
}

#[component]
pub fn BwTsChart(props: BwTsChartProps) -> Element {
    let data = &props.data;

    // Chart dimensions
    let width = 800.0;
    let height = 300.0;
    let padding = 60.0;
    let chart_width = width - padding * 2.0;
    let chart_height = height - padding * 2.0;

    // Find max value for scaling
    let max_value = data.iter().map(|d| d.send.max(d.recv)).max().unwrap_or(0) as f64;

    // Generate paths
    let send_path = generate_path(data, |d| d.send, chart_width, chart_height, max_value);
    let recv_path = generate_path(data, |d| d.recv, chart_width, chart_height, max_value);

    // Generate Y-axis labels (5 ticks)
    let y_labels: Vec<_> = (0..=4)
        .map(|i| {
            let value = (max_value as f64 / 4.0 * (4 - i) as f64) as u64;
            let y = padding + (chart_height / 4.0 * i as f64);
            (humanize_bytes(value), y)
        })
        .collect();

    // Generate X-axis labels (show every ~20th data point, max 6 labels)
    let x_labels: Vec<_> = if !data.is_empty() {
        let step = (data.len() / 5).max(1);
        data.iter()
            .enumerate()
            .step_by(step)
            .map(|(i, point)| {
                let x = padding + (i as f64 / (data.len() - 1).max(1) as f64) * chart_width;
                let time_str = point.ts.format("%H:%M:%S").to_string();
                (time_str, x)
            })
            .collect()
    } else {
        vec![]
    };

    rsx! {
        div {
            class: "p-4",
            h2 {
                class: "text-xl font-bold mb-4",
                "Bandwidth"
            }

            if data.is_empty() {
                div {
                    class: "text-gray-500 text-center py-8",
                    "No data available"
                }
            } else {
                div {
                    class: "flex gap-4 mb-2",
                    div {
                        class: "flex items-center gap-2",
                        div {
                            class: "w-4 h-0.5",
                            style: "background-color: #3b82f6;",
                        }
                        span {
                            class: "text-sm",
                            "Send"
                        }
                    }
                    div {
                        class: "flex items-center gap-2",
                        div {
                            class: "w-4 h-0.5",
                            style: "background-color: #10b981;",
                        }
                        span {
                            class: "text-sm",
                            "Receive"
                        }
                    }
                }

                svg {
                    width: "{width}",
                    height: "{height}",
                    view_box: "0 0 {width} {height}",

                    // Y-axis
                    line {
                        x1: "{padding}",
                        y1: "{padding}",
                        x2: "{padding}",
                        y2: "{height - padding}",
                        stroke: "#666",
                        stroke_width: "1",
                    }

                    // X-axis
                    line {
                        x1: "{padding}",
                        y1: "{height - padding}",
                        x2: "{width - padding}",
                        y2: "{height - padding}",
                        stroke: "#666",
                        stroke_width: "1",
                    }

                    // Y-axis labels and grid lines
                    for (label, y) in y_labels {
                        g {
                            // Grid line
                            line {
                                x1: "{padding}",
                                y1: "{y}",
                                x2: "{width - padding}",
                                y2: "{y}",
                                stroke: "#333",
                                stroke_width: "0.5",
                                stroke_dasharray: "2,2",
                            }
                            // Label
                            text {
                                x: "{padding - 10.0}",
                                y: "{y + 5.0}",
                                text_anchor: "end",
                                font_size: "12",
                                fill: "#999",
                                "{label}"
                            }
                        }
                    }

                    // X-axis labels
                    for (label, x) in x_labels {
                        text {
                            x: "{x}",
                            y: "{height - padding + 20.0}",
                            text_anchor: "middle",
                            font_size: "10",
                            fill: "#999",
                            "{label}"
                        }
                    }

                    // Chart area group
                    g {
                        transform: "translate({padding}, {padding})",

                        // Send line (blue)
                        path {
                            d: "{send_path}",
                            fill: "none",
                            stroke: "#3b82f6",
                            stroke_width: "2",
                        }

                        // Receive line (green)
                        path {
                            d: "{recv_path}",
                            fill: "none",
                            stroke: "#10b981",
                            stroke_width: "2",
                        }
                    }

                    // X-axis label
                    text {
                        x: "{width / 2.0}",
                        y: "{height - 10.0}",
                        text_anchor: "middle",
                        font_size: "12",
                        fill: "#999",
                        "Time"
                    }
                }
            }
        }
    }
}

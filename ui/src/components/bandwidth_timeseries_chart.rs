use dioxus::prelude::*;
use n0_future::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub ts: SystemTime,
    pub send: u64,
    pub recv: u64,
}

#[derive(PartialEq, Clone, Props)]
pub struct BwTsChartProps {
    pub data: Vec<ChartData>,
}

const CLASS: &str = "py-2 px-5 border-1 border-white rounded-md";

#[component]
pub fn BwTsChart(props: BwTsChartProps) -> Element {
    rsx! {
        h1{ "bandwidth" }

    }
}

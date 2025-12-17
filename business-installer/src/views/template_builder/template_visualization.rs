use crate::state::{message::Msg, State};
use iced::Length;
use liana_ui::widget::*;
use std::fmt::Write;

const PATH_SPACING: f32 = 90.0;
const PATH_BOX_WIDTH: f32 = 400.0;

// Colors for paths
const PRIMARY_COLOR: &str = "#32cd32"; // Green

// Generate color from green to blue gradient based on index and total count
// Recovery path 1 should be between green and blue (not the same as primary green)
fn get_secondary_color(index: usize, total_count: usize) -> String {
    if total_count == 0 {
        return "#32cd32".to_string(); // Default to green
    }

    // Calculate interpolation factor (0.0 = mid-green-blue, 1.0 = blue)
    // First recovery path (index 0) should be between green and blue
    // Last recovery path should be blue
    let factor = if total_count == 1 {
        0.5 // Single recovery path: midpoint between green and blue
    } else {
        // Distribute from 0.0 (first) to 1.0 (last)
        // This ensures first path is between green and blue, not pure green
        index as f32 / (total_count - 1) as f32
    };

    // Start color (mid-green-blue): RGB(25, 102, 152) - halfway between green and blue
    // End color (blue): RGB(0, 0, 255) = #0000ff
    // Primary green: RGB(50, 205, 50) = #32cd32
    // Midpoint: RGB(25, 102, 152) = #196698

    let start_r = 25.0;
    let start_g = 102.0;
    let start_b = 152.0;

    let end_r = 0.0;
    let end_g = 0.0;
    let end_b = 255.0;

    // Interpolate from mid-green-blue to blue
    let r = (start_r + (end_r - start_r) * factor) as u8;
    let g = (start_g + (end_g - start_g) * factor) as u8;
    let b = (start_b + (end_b - start_b) * factor) as u8;

    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

pub fn template_visualization(state: &State) -> Element<'static, Msg> {
    let svg_content = generate_svg(state);

    let svg_handle = iced::widget::svg::Handle::from_memory(svg_content.as_bytes().to_vec());
    let svg_widget = liana_ui::widget::Svg::new(svg_handle)
        .width(Length::Fill)
        .height(Length::Fill)
        .content_fit(iced::ContentFit::Contain);

    Container::new(svg_widget)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20.0)
        .into()
}

fn generate_svg(state: &State) -> String {
    let mut svg = String::new();
    let primary_path = &state.app.primary_path;
    let secondary_paths = &state.app.secondary_paths;

    // Calculate total height needed
    let num_paths = 1 + secondary_paths.len();
    let total_height = if num_paths == 0 {
        200.0 // Minimum height
    } else {
        (num_paths as f32) * PATH_SPACING
    };

    // Start SVG with viewBox for better scaling
    write!(
        svg,
        r#"<svg width="{}" height="{}" viewBox="0 0 {} {}" xmlns="http://www.w3.org/2000/svg">"#,
        PATH_BOX_WIDTH, total_height, PATH_BOX_WIDTH, total_height
    )
    .unwrap();

    // Primary path
    let y_pos = PATH_SPACING / 2.0;
    render_path_box(
        &mut svg,
        y_pos,
        PRIMARY_COLOR,
        "Primary path",
        primary_path,
        &state.app.keys,
    );

    // Secondary paths
    let total_recovery_paths = secondary_paths.len();
    for (index, (path, _timelock)) in secondary_paths.iter().enumerate() {
        let y_pos = PATH_SPACING * (index as f32 + 1.5);
        let color = get_secondary_color(index, total_recovery_paths);
        let label = if index == 0 {
            "Secondary path 1".to_string()
        } else {
            format!("Secondary path {}", index + 1)
        };

        render_path_box(&mut svg, y_pos, &color, &label, path, &state.app.keys);
    }

    // Close SVG
    svg.push_str("</svg>");

    svg
}

fn render_path_box(
    svg: &mut String,
    y: f32,
    color: &str,
    label: &str,
    _path: &liana_connect::SpendingPath,
    _keys: &std::collections::BTreeMap<u8, liana_connect::Key>,
) {
    let box_y = y;

    // Render the "r" shape on the left
    let r_shape_x = 30.0;
    render_r_shape(svg, r_shape_x, box_y, color);

    // Render label text to the right of the "r" shape
    let text_x = 120.0;
    let text_y = box_y;

    // Split label to underline "path" part
    if let Some(path_idx) = label.find("path") {
        let before_path = &label[..path_idx];
        let path_part = "path";
        let after_path = &label[path_idx + 4..];

        // Render text before "path"
        if !before_path.is_empty() {
            write!(
                svg,
                r#"<text x="{}" y="{}" fill="white" font-family="sans-serif" font-size="16" font-weight="400" dominant-baseline="middle">{}</text>"#,
                text_x,
                text_y,
                escape_xml(before_path)
            ).unwrap();
        }

        // Calculate width of text before "path" (approximate: 8 pixels per character for font-size 16)
        let before_width = before_path.len() as f32 * 8.0;
        let path_x = text_x + before_width;

        // Render "path" text
        let path_width = path_part.len() as f32 * 8.0;
        write!(
            svg,
            r#"<text x="{}" y="{}" fill="white" font-family="sans-serif" font-size="16" font-weight="400" dominant-baseline="middle">{}</text>"#,
            path_x,
            text_y,
            escape_xml(path_part)
        ).unwrap();

        // Render text after "path" (like " 1")
        if !after_path.is_empty() {
            let path_total_width = before_width + path_width;
            write!(
                svg,
                r#"<text x="{}" y="{}" fill="white" font-family="sans-serif" font-size="16" font-weight="400" dominant-baseline="middle">{}</text>"#,
                text_x + path_total_width,
                text_y,
                escape_xml(after_path)
            ).unwrap();
        }
    } else {
        // If no "path" found, just render the whole label
        write!(
            svg,
            r#"<text x="{}" y="{}" fill="white" font-family="sans-serif" font-size="16" font-weight="400" dominant-baseline="middle">{}</text>"#,
            text_x,
            text_y,
            escape_xml(label)
        ).unwrap();
    }
}

fn render_r_shape(svg: &mut String, x: f32, y: f32, color: &str) {
    // Create a stylized lowercase "r" with just 2 elements: a line and a radius
    // Element 1: Vertical stem (line)
    let thickness = 10.0;
    let stem_top = y - 20.0;
    let stem_bottom = y + 25.0;

    // Element 2: Radius/arc that extends right from middle-top and curves down
    let radius = 25.0; // Regular radius for the circular arc
                       //  arc_start_x = 0
    let arc_start_y = y + 25.0; // Where the arc starts
    let arc_end_x = x + 25.0;
    let arc_end_y = y;

    // Element 1: Vertical line (the stem)
    write!(
        svg,
        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round" />"#,
        x,
        stem_top,
        x,
        stem_bottom,
        color,
        thickness
    ).unwrap();

    // Element 2: Single arc with regular radius
    // The arc extends horizontally from the stem, then curves down
    // Using SVG arc command for a true circular arc with regular radius

    write!(
        svg,
        r#"<path d="M {} {} A {} {} 0 0 1 {} {}" stroke="{}" stroke-width="{}" fill="none" stroke-linecap="round" />"#,
        // Start point: where arc begins (middle-top of stem)
        x,
        arc_start_y,
        // Radius X and Y (circular arc, so same radius)
        radius,
        radius,
        // End point: bottom of the tail
        arc_end_x,
        arc_end_y,
        color,
        thickness
    ).unwrap();
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}


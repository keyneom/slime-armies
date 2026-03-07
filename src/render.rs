use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::game::{Game, Scene, MenuSelection};
use crate::entities::{Player, Spider, Cannon, Snake, Wisp, Projectile};
use crate::math::Vec2;
use crate::world::{Camera, CHUNK_SIZE};
use crate::net::{NetworkSession, NetworkState, RemotePlayer, PlayerStats};

// Color palette - exact match to original WASM-4 game
// Palette: [0x000011, 0xdddd99, 0x1166cc, 0xcc1166]
const COLOR_1: &str = "#000011";  // Color 1: Dark blue/black (background)
const COLOR_2: &str = "#dddd99";  // Color 2: Light cream/yellow (player eyes, shield lines)
const COLOR_3: &str = "#1166cc";  // Color 3: Blue (attack effects, phasing)
const COLOR_4: &str = "#cc1166";  // Color 4: Magenta/pink (player body, enemies)

// Aliases for clarity
const COLOR_BG: &str = COLOR_1;
const COLOR_LIGHT: &str = COLOR_2;
const COLOR_ACCENT1: &str = COLOR_3;  // Blue
const COLOR_ACCENT2: &str = COLOR_4;  // Magenta/pink (same as enemy color!)
const COLOR_OBSTACLE: &str = "#334444";  // Dark teal for rocks/obstacles
const COLOR_REMOTE_PLAYER: &str = "#66cc66";  // Green for remote players

// Pixelation scale (1.0 keeps world sizes accurate; pixel grid handled in renderer)
const RENDER_SCALE: f64 = 1.0;
// Creature render scale (increase to make all creatures larger without zoom)
const CREATURE_SCALE: f64 = 2.0;

#[derive(Clone)]
struct PlayerEntry {
    name: String,
    kills: u32,
    deaths: u32,
    time_seconds: u32,
    score: u32,
}

pub struct Renderer {
    ctx: CanvasRenderingContext2d,
    display_ctx: CanvasRenderingContext2d,
    render_canvas: HtmlCanvasElement,
    width: u32,
    height: u32,
    render_width: u32,
    render_height: u32,
    render_scale: f64,
    scale_x: f64,
    scale_y: f64,
}

impl Renderer {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Self, JsValue> {
        let display_ctx = canvas
            .get_context("2d")?
            .ok_or("Failed to get 2d context")?
            .dyn_into::<CanvasRenderingContext2d>()?;

        // Low-res offscreen canvas for pixelated rendering (scaled up to display)
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("Failed to access document")?;
        let render_canvas = document
            .create_element("canvas")?
            .dyn_into::<HtmlCanvasElement>()?;

        let width = canvas.width();
        let height = canvas.height();
        let render_scale = RENDER_SCALE;
        let render_width = ((width as f64) / render_scale).round() as u32;
        let render_height = ((height as f64) / render_scale).round() as u32;
        render_canvas.set_width(render_width);
        render_canvas.set_height(render_height);

        let ctx = render_canvas
            .get_context("2d")?
            .ok_or("Failed to get offscreen 2d context")?
            .dyn_into::<CanvasRenderingContext2d>()?;

        // Disable smoothing when scaling the low-res buffer to the display
        display_ctx.set_image_smoothing_enabled(false);
        ctx.set_image_smoothing_enabled(false);

        let scale_x = render_width as f64 / width as f64;
        let scale_y = render_height as f64 / height as f64;

        Ok(Self {
            ctx,
            display_ctx,
            render_canvas,
            width,
            height,
            render_width,
            render_height,
            render_scale,
            scale_x,
            scale_y,
        })
    }

    pub fn render(&self, game: &Game, network: &NetworkSession) {
        // Clear low-res buffer and set scale so existing coordinates map down
        let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        self.ctx.set_global_alpha(1.0);
        self.ctx.set_fill_style_str(COLOR_BG);
        self.ctx.fill_rect(0.0, 0.0, self.render_width as f64, self.render_height as f64);
        let _ = self.ctx.set_transform(self.scale_x, 0.0, 0.0, self.scale_y, 0.0, 0.0);

        match game.scene {
            Scene::Title => self.render_title(game, network),
            Scene::Game => self.render_game(game, network),
            Scene::GameOver => self.render_gameover(game),
        }

        // Blit low-res buffer to display canvas (nearest-neighbor)
        let _ = self.display_ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        self.display_ctx
            .clear_rect(0.0, 0.0, self.width as f64, self.height as f64);
        let _ = self.display_ctx.draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &self.render_canvas,
            0.0,
            0.0,
            self.render_width as f64,
            self.render_height as f64,
            0.0,
            0.0,
            self.width as f64,
            self.height as f64,
        );

        // Overlay UI at full resolution so text stays readable
        match game.scene {
            Scene::Title => self.render_title_overlay(game, network),
            Scene::Game => self.render_game_overlay(game, network),
            Scene::GameOver => self.render_gameover_overlay(game, network),
        }
    }

    fn snap_point(&self, x: f64, y: f64) -> (f64, f64) {
        let sx = (x * self.scale_x).round() / self.scale_x;
        let sy = (y * self.scale_y).round() / self.scale_y;
        (sx, sy)
    }

    fn world_to_render_pixel(&self, value: f64) -> i32 {
        (value * self.scale_x).round() as i32
    }

    fn world_to_render_size(&self, size: f64) -> i32 {
        (size * self.scale_x).round().max(1.0) as i32
    }

    fn render_title(&self, game: &Game, network: &NetworkSession) {
        let center_x = (self.width / 2) as f64;

        // Draw title slime
        self.ctx.set_fill_style_str(COLOR_ACCENT1);
        self.fill_oval(center_x, 120.0, 100.0, 80.0);

        // Title text
        self.ctx.set_fill_style_str(COLOR_ACCENT2);
        self.ctx.set_font("bold 36px monospace");
        self.ctx.set_text_align("center");
        let _ = self.ctx.fill_text("SLIME ARMIES", center_x, 60.0);

        // Player name input
        let name_y = 220.0;
        self.ctx.set_font("14px monospace");
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        self.ctx.set_text_align("right");
        let _ = self.ctx.fill_text("Name:", center_x - 100.0, name_y);

        // Name input box
        let is_name_active = game.text_input_active && game.active_input_field == 0;
        self.render_text_input(center_x - 90.0, name_y - 14.0, 180.0, &game.player_name, is_name_active, game.frame_count);

        // Room code input
        let code_y = 260.0;
        self.ctx.set_text_align("right");
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        let _ = self.ctx.fill_text("Room:", center_x - 100.0, code_y);

        // Room code input box
        let is_code_active = game.text_input_active && game.active_input_field == 1;
        self.render_text_input(center_x - 90.0, code_y - 14.0, 100.0, &game.room_code_input, is_code_active, game.frame_count);

        // Menu options
        let menu_y = 320.0;
        let menu_spacing = 40.0;

        self.render_menu_item(center_x, menu_y, "SOLO PLAY", game.menu_selection == MenuSelection::Play, game.frame_count);
        self.render_menu_item(center_x, menu_y + menu_spacing, "CREATE ROOM", game.menu_selection == MenuSelection::CreateRoom, game.frame_count);

        // Network status
        self.render_network_status(network);

    }

    fn render_text_input(&self, x: f64, y: f64, width: f64, text: &str, active: bool, frame_count: u32) {
        // Background
        let bg_color = if active { "#222233" } else { "#111122" };
        self.ctx.set_fill_style_str(bg_color);
        self.ctx.fill_rect(x, y, width, 22.0);

        // Border
        let border_color = if active { COLOR_ACCENT1 } else { "#334466" };
        self.ctx.set_stroke_style_str(border_color);
        self.ctx.set_line_width(1.0);
        self.ctx.stroke_rect(x, y, width, 22.0);

        // Text
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        self.ctx.set_font("14px monospace");
        self.ctx.set_text_align("left");

        let display_text = if text.is_empty() && !active {
            "..."
        } else {
            text
        };

        let _ = self.ctx.fill_text(display_text, x + 5.0, y + 16.0);

        // Cursor (blinking)
        if active && (frame_count / 30) % 2 == 0 {
            let cursor_x = x + 5.0 + (text.len() as f64 * 8.4);
            self.ctx.set_fill_style_str(COLOR_LIGHT);
            self.ctx.fill_rect(cursor_x, y + 4.0, 2.0, 14.0);
        }
    }

    fn render_menu_item(&self, x: f64, y: f64, text: &str, selected: bool, frame_count: u32) {
        self.ctx.set_font("18px monospace");
        self.ctx.set_text_align("center");

        if selected {
            // Pulsing effect for selected item
            let pulse = ((frame_count as f64 * 0.1).sin() * 0.3 + 0.7) as f64;
            self.ctx.set_global_alpha(pulse);
            self.ctx.set_fill_style_str(COLOR_ACCENT2);

            // Selection indicator
            let _ = self.ctx.fill_text(">", x - 80.0, y);
            let _ = self.ctx.fill_text("<", x + 80.0, y);

            self.ctx.set_global_alpha(1.0);
            self.ctx.set_fill_style_str(COLOR_ACCENT2);
        } else {
            self.ctx.set_fill_style_str(COLOR_LIGHT);
        }

        let _ = self.ctx.fill_text(text, x, y);
    }

    fn render_game(&self, game: &Game, network: &NetworkSession) {
        let camera = &game.camera;

        // Draw terrain/obstacles from chunks
        self.render_terrain(game);

        // Draw shrine structure
        self.render_shrine(game);

        // Draw slime trail segments
        if !game.trail_segments.is_empty() {
            self.ctx.set_fill_style_str("#44aa88");
            for segment in &game.trail_segments {
                let screen_pos = camera.world_to_screen(segment.pos());
                self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, segment.radius() as f64);
            }
        }

        if let Some(snapshot) = &game.enemy_render_snapshot {
            // Draw snakes (back to front so head is on top)
            for i in (0..snapshot.snake_positions.len()).rev() {
                let (pos, dir, size, alive) = snapshot.snake_positions[i];
                if alive {
                    self.render_snake_snapshot(i, pos, dir, size, game.frame_count, camera);
                }
            }

            // Draw spiders
            for (i, (pos, dir, alive)) in snapshot.spider_positions.iter().enumerate() {
                if *alive {
                    self.render_spider_snapshot(i, *pos, *dir, game.frame_count, camera);
                }
            }

            // Draw cannons
            for (i, (pos, dir, look_dir, alive)) in snapshot.cannon_positions.iter().enumerate() {
                if *alive {
                    self.render_cannon_snapshot(i, *pos, *dir, *look_dir, camera);
                }
            }

            // Draw wisps
            for (i, (pos, dir, alive)) in snapshot.wisp_positions.iter().enumerate() {
                if *alive {
                    self.render_wisp_snapshot(i, *pos, *dir, game.frame_count, camera);
                }
            }
        } else {
            // Draw snakes (back to front so head is on top)
            for i in (0..game.snakes.len()).rev() {
                if game.snakes[i].alive {
                    self.render_snake(&game.snakes[i], game.frame_count, camera);
                }
            }

            // Draw spiders
            for spider in &game.spiders {
                if spider.alive {
                    self.render_spider(spider, game.frame_count, camera);
                }
            }

            // Draw cannons
            for cannon in &game.cannons {
                if cannon.alive {
                    self.render_cannon(cannon, camera);
                }
            }

            // Draw wisps
            for wisp in &game.wisps {
                if wisp.alive {
                    self.render_wisp(wisp, game.frame_count, camera);
                }
            }
        }
        for guardian in &game.guardians {
            if guardian.alive {
                self.render_guardian(guardian, game.frame_count, camera);
            }
        }

        // Draw projectiles
        for projectile in game.projectiles.iter() {
            self.render_projectile(projectile, camera);
        }

        if game.shockwave_timer > 0 {
            let progress = 1.0 - (game.shockwave_timer as f64 / 12.0);
            let radius = self.world_to_render_size((20.0 + progress as f32 * 70.0) as f64 * CREATURE_SCALE) as f64;
            let screen_pos = camera.world_to_screen(game.player.pos);
            let render_x = self.world_to_render_pixel(screen_pos.x as f64) as f64;
            let render_y = self.world_to_render_pixel(screen_pos.y as f64) as f64;
            self.ctx.set_stroke_style_str(COLOR_LIGHT);
            self.ctx.set_global_alpha(0.6);
            self.draw_circle_outline(render_x, render_y, (radius / 2.0).ceil());
            self.ctx.set_global_alpha(1.0);
        }

        // Draw remote players
        for remote in network.remote_players.values() {
            if remote.alive {
                self.render_remote_player(remote, camera);
            }
        }

        // Draw player
        if game.player.alive {
            self.render_player(&game.player, camera);
        }

        // Draw explosions
        for explosion in game.explosions.iter() {
            let color = match explosion.color_index {
                0 => COLOR_ACCENT2,
                _ => COLOR_BG,
            };
            let screen_pos = camera.world_to_screen(explosion.pos);
            self.ctx.set_fill_style_str(color);
            self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, explosion.radius() as f64);
        }

        // HUD and names are drawn in overlay for readability
    }

    fn render_shrine(&self, game: &Game) {
        let camera = &game.camera;
        let shrine_pos = crate::game::SHRINE_POS;
        let screen_pos = camera.world_to_screen(shrine_pos);
        let (min_x, min_y, max_x, max_y) = camera.visible_bounds();
        if shrine_pos.x < min_x - 120.0 || shrine_pos.x > max_x + 120.0 || shrine_pos.y < min_y - 120.0 || shrine_pos.y > max_y + 120.0 {
            return;
        }

        let base_radius = 18.0;
        let glow = if game.shrine_triggered() { 0.8 } else { 0.4 };
        let pulse = ((game.frame_count as f64 * 0.06).sin() * 0.5 + 0.5) * 0.6 + 0.4;

        self.ctx.set_global_alpha(glow);
        self.ctx.set_fill_style_str("#88c9ff");
        self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, base_radius + 10.0);
        self.ctx.set_global_alpha(1.0);

        self.ctx.set_fill_style_str("#243a4a");
        self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, base_radius + 4.0);

        self.ctx.set_fill_style_str("#8cd4ff");
        self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, base_radius);

        self.ctx.set_fill_style_str("#0f1820");
        self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, base_radius * 0.5);

        self.ctx.set_stroke_style_str("#bfe8ff");
        self.ctx.set_line_width(2.0);
        self.ctx.begin_path();
        let _ = self.ctx.arc(
            screen_pos.x as f64,
            screen_pos.y as f64,
            base_radius as f64 * 1.35,
            0.0,
            std::f64::consts::TAU,
        );
        self.ctx.stroke();

        self.ctx.set_global_alpha(pulse);
        self.ctx.set_stroke_style_str("#c7f1ff");
        self.ctx.set_line_width(1.5);
        self.ctx.begin_path();
        let _ = self.ctx.arc(
            screen_pos.x as f64,
            screen_pos.y as f64,
            base_radius as f64 * 1.7,
            0.0,
            std::f64::consts::TAU,
        );
        self.ctx.stroke();
        self.ctx.set_global_alpha(1.0);

        let pillar_offset = base_radius as f64 * 1.6;
        let pillar_size = 6.0;
        self.ctx.set_fill_style_str("#1d2a33");
        self.ctx.fill_rect(screen_pos.x as f64 - pillar_offset - pillar_size / 2.0, screen_pos.y as f64 - pillar_size / 2.0, pillar_size, pillar_size);
        self.ctx.fill_rect(screen_pos.x as f64 + pillar_offset - pillar_size / 2.0, screen_pos.y as f64 - pillar_size / 2.0, pillar_size, pillar_size);
        self.ctx.fill_rect(screen_pos.x as f64 - pillar_size / 2.0, screen_pos.y as f64 - pillar_offset - pillar_size / 2.0, pillar_size, pillar_size);
        self.ctx.fill_rect(screen_pos.x as f64 - pillar_size / 2.0, screen_pos.y as f64 + pillar_offset - pillar_size / 2.0, pillar_size, pillar_size);
    }

    fn render_terrain(&self, game: &Game) {
        let camera = &game.camera;
        let (min_x, min_y, max_x, max_y) = camera.visible_bounds();

        // Draw visible obstacles (rocks)
        let obstacles = game.chunks.visible_obstacles(min_x, min_y, max_x, max_y);
        for obstacle in obstacles {
            let screen_pos = camera.world_to_screen(obstacle.pos);
            let color = match obstacle.variant {
                0 => COLOR_OBSTACLE,
                1 => "#445544",  // Mossy rock
                _ => "#333344",  // Blue-gray rock
            };
            self.ctx.set_fill_style_str(color);
            self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, obstacle.radius as f64);

            // Add a highlight
            self.ctx.set_fill_style_str("#556666");
            self.fill_circle(
                screen_pos.x as f64 - obstacle.radius as f64 * 0.2,
                screen_pos.y as f64 - obstacle.radius as f64 * 0.2,
                obstacle.radius as f64 * 0.3,
            );
        }
    }

    fn render_spider(&self, spider: &Spider, frame_count: u32, camera: &Camera) {
        let screen_pos = camera.world_to_screen(spider.pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let flip: f64 = if (frame_count / 5 + spider.id as u32) % 2 == 0 { 1.0 } else { -1.0 };
        let scale = CREATURE_SCALE;

        // Use dir for leg orientation - ensure we have a valid direction
        let (dir_x, dir_y) = if spider.dir.x == 0.0 && spider.dir.y == 0.0 {
            (0.0, 1.0) // Default to facing down if no direction
        } else {
            (spider.dir.x as f64, spider.dir.y as f64)
        };

        // Match original WASM-4 sizing (drawCircle size 5, legs 4, offset 2)
        let body_radius = 2.5 * scale;
        let leg_length = 4.0 * scale;
        let leg_offset = 2.0 * scale;

        // DRAW_COLORS = 4 (magenta/pink)
        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * scale).max(1.0));

        // Legs - two pairs that alternate with animation
        // First leg pair: diagonal one way
        self.ctx.begin_path();
        let (s1x, s1y) = self.snap_point(
            x + (dir_x * (leg_offset + flip) + dir_y * leg_length),
            y + (dir_y * (leg_offset + flip) - dir_x * leg_length),
        );
        let (e1x, e1y) = self.snap_point(
            x - (dir_x * (leg_offset + flip) + dir_y * leg_length),
            y - (dir_y * (leg_offset + flip) - dir_x * leg_length),
        );
        self.ctx.move_to(s1x, s1y);
        self.ctx.line_to(e1x, e1y);
        self.ctx.stroke();

        // Second leg pair: diagonal other way
        self.ctx.begin_path();
        let (s2x, s2y) = self.snap_point(
            x + (dir_x * (leg_offset - flip) - dir_y * leg_length),
            y + (dir_y * (leg_offset - flip) + dir_x * leg_length),
        );
        let (e2x, e2y) = self.snap_point(
            x - (dir_x * (leg_offset - flip) - dir_y * leg_length),
            y - (dir_y * (leg_offset - flip) + dir_x * leg_length),
        );
        self.ctx.move_to(s2x, s2y);
        self.ctx.line_to(e2x, e2y);
        self.ctx.stroke();

        // Body - filled circle
        self.ctx.set_fill_style_str(COLOR_4);
        self.fill_circle(x, y, body_radius);

        // Eyes - single pixels at dir * 4 +/- perp * 1.5
        let (eye1_x, eye1_y) = self.snap_point(
            x + dir_x * 4.0 + dir_y * 1.5,
            y + dir_y * 4.0 - dir_x * 1.5,
        );
        let (eye2_x, eye2_y) = self.snap_point(
            x + dir_x * 4.0 - dir_y * 1.5,
            y + dir_y * 4.0 + dir_x * 1.5,
        );
        self.ctx.fill_rect(eye1_x.round(), eye1_y.round(), 1.0, 1.0);
        self.ctx.fill_rect(eye2_x.round(), eye2_y.round(), 1.0, 1.0);
    }

    fn render_spider_snapshot(&self, id: usize, pos: Vec2, dir: Vec2, frame_count: u32, camera: &Camera) {
        let screen_pos = camera.world_to_screen(pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let flip: f64 = if (frame_count / 5 + id as u32) % 2 == 0 { 1.0 } else { -1.0 };
        let scale = CREATURE_SCALE;

        let (dir_x, dir_y) = if dir.x == 0.0 && dir.y == 0.0 {
            (0.0, 1.0)
        } else {
            (dir.x as f64, dir.y as f64)
        };

        let body_radius = 2.5 * scale;
        let leg_length = 4.0 * scale;
        let leg_offset = 2.0 * scale;

        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * scale).max(1.0));

        self.ctx.begin_path();
        let (s1x, s1y) = self.snap_point(
            x + (dir_x * (leg_offset + flip) + dir_y * leg_length),
            y + (dir_y * (leg_offset + flip) - dir_x * leg_length),
        );
        let (e1x, e1y) = self.snap_point(
            x - (dir_x * (leg_offset + flip) + dir_y * leg_length),
            y - (dir_y * (leg_offset + flip) - dir_x * leg_length),
        );
        self.ctx.move_to(s1x, s1y);
        self.ctx.line_to(e1x, e1y);
        self.ctx.stroke();

        self.ctx.begin_path();
        let (s2x, s2y) = self.snap_point(
            x + (dir_x * (leg_offset - flip) - dir_y * leg_length),
            y + (dir_y * (leg_offset - flip) + dir_x * leg_length),
        );
        let (e2x, e2y) = self.snap_point(
            x - (dir_x * (leg_offset - flip) - dir_y * leg_length),
            y - (dir_y * (leg_offset - flip) + dir_x * leg_length),
        );
        self.ctx.move_to(s2x, s2y);
        self.ctx.line_to(e2x, e2y);
        self.ctx.stroke();

        self.ctx.set_fill_style_str(COLOR_4);
        self.fill_circle(x, y, body_radius);

        let (eye1_x, eye1_y) = self.snap_point(
            x + dir_x * 4.0 * scale + dir_y * 1.5 * scale,
            y + dir_y * 4.0 * scale - dir_x * 1.5 * scale,
        );
        let (eye2_x, eye2_y) = self.snap_point(
            x + dir_x * 4.0 * scale - dir_y * 1.5 * scale,
            y + dir_y * 4.0 * scale + dir_x * 1.5 * scale,
        );
        let eye_size = (1.0 * scale).round().max(1.0);
        self.ctx.fill_rect(eye1_x.round(), eye1_y.round(), eye_size, eye_size);
        self.ctx.fill_rect(eye2_x.round(), eye2_y.round(), eye_size, eye_size);
    }

    fn render_cannon(&self, cannon: &Cannon, camera: &Camera) {
        let screen_pos = camera.world_to_screen(cannon.pos);
        let x = screen_pos.x as f64;
        let y = screen_pos.y as f64;
        let scale = CREATURE_SCALE;

        // Wheels - DRAW_COLORS = 0x33 (blue)
        self.ctx.set_fill_style_str(COLOR_3);
        self.fill_circle(x + cannon.dir.y as f64 * 4.0 * scale, y - cannon.dir.x as f64 * 4.0 * scale, 2.0 * scale);
        self.fill_circle(x - cannon.dir.y as f64 * 4.0 * scale, y + cannon.dir.x as f64 * 4.0 * scale, 2.0 * scale);

        // Barrel - DRAW_COLORS = 4 (magenta/pink)
        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * scale).max(1.0));

        // Center line (-3.5 to +4)
        self.ctx.begin_path();
        self.ctx.move_to(
            x - cannon.look_dir.x as f64 * 3.5 * scale,
            y - cannon.look_dir.y as f64 * 3.5 * scale,
        );
        self.ctx.line_to(
            x + cannon.look_dir.x as f64 * 4.0 * scale,
            y + cannon.look_dir.y as f64 * 4.0 * scale,
        );
        self.ctx.stroke();

        // Top line (-3 to +5, offset by perpendicular 0.5)
        self.ctx.begin_path();
        self.ctx.move_to(
            x - cannon.look_dir.x as f64 * 3.0 * scale - cannon.look_dir.y as f64 * 0.5 * scale,
            y - cannon.look_dir.y as f64 * 3.0 * scale + cannon.look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.line_to(
            x + cannon.look_dir.x as f64 * 5.0 * scale - cannon.look_dir.y as f64 * 0.5 * scale,
            y + cannon.look_dir.y as f64 * 5.0 * scale + cannon.look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.stroke();

        // Bottom line (-3 to +5, offset by perpendicular -0.5)
        self.ctx.begin_path();
        self.ctx.move_to(
            x - cannon.look_dir.x as f64 * 3.0 * scale + cannon.look_dir.y as f64 * 0.5 * scale,
            y - cannon.look_dir.y as f64 * 3.0 * scale - cannon.look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.line_to(
            x + cannon.look_dir.x as f64 * 5.0 * scale + cannon.look_dir.y as f64 * 0.5 * scale,
            y + cannon.look_dir.y as f64 * 5.0 * scale - cannon.look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.stroke();
    }

    fn render_cannon_snapshot(&self, _id: usize, pos: Vec2, dir: Vec2, look_dir: Vec2, camera: &Camera) {
        let screen_pos = camera.world_to_screen(pos);
        let x = screen_pos.x as f64;
        let y = screen_pos.y as f64;
        let scale = CREATURE_SCALE;

        self.ctx.set_fill_style_str(COLOR_3);
        self.fill_circle(x + dir.y as f64 * 4.0 * scale, y - dir.x as f64 * 4.0 * scale, 2.0 * scale);
        self.fill_circle(x - dir.y as f64 * 4.0 * scale, y + dir.x as f64 * 4.0 * scale, 2.0 * scale);

        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * scale).max(1.0));

        self.ctx.begin_path();
        self.ctx.move_to(
            x - look_dir.x as f64 * 3.5 * scale,
            y - look_dir.y as f64 * 3.5 * scale,
        );
        self.ctx.line_to(
            x + look_dir.x as f64 * 4.0 * scale,
            y + look_dir.y as f64 * 4.0 * scale,
        );
        self.ctx.stroke();

        self.ctx.begin_path();
        self.ctx.move_to(
            x - look_dir.x as f64 * 3.0 * scale - look_dir.y as f64 * 0.5 * scale,
            y - look_dir.y as f64 * 3.0 * scale + look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.line_to(
            x + look_dir.x as f64 * 5.0 * scale - look_dir.y as f64 * 0.5 * scale,
            y + look_dir.y as f64 * 5.0 * scale + look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.stroke();

        self.ctx.begin_path();
        self.ctx.move_to(
            x - look_dir.x as f64 * 3.0 * scale + look_dir.y as f64 * 0.5 * scale,
            y - look_dir.y as f64 * 3.0 * scale - look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.line_to(
            x + look_dir.x as f64 * 5.0 * scale + look_dir.y as f64 * 0.5 * scale,
            y + look_dir.y as f64 * 5.0 * scale - look_dir.x as f64 * 0.5 * scale,
        );
        self.ctx.stroke();
    }

    fn render_snake(&self, snake: &Snake, frame_count: u32, camera: &Camera) {
        let screen_pos = camera.world_to_screen(snake.pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let size = snake.size as f64 * CREATURE_SCALE;
        self.ctx.set_global_alpha(1.0);
        let flip: f64 = if (frame_count / 5 + snake.id as u32) % 2 == 0 { 1.0 } else { -1.0 };
        let (dx, dy) = if snake.dir.x == 0.0 && snake.dir.y == 0.0 {
            (0.0, 1.0)
        } else {
            (snake.dir.x as f64, snake.dir.y as f64)
        };

        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * CREATURE_SCALE).max(1.0));

        // Legs first so body fill masks inner parts
        let leg_scale = 1.15;
        let (s1x, s1y) = self.snap_point(
            x + (dx * (2.5 + flip) * 0.1 + dy * leg_scale) * size,
            y + (dy * (2.5 + flip) * 0.1 - dx * leg_scale) * size,
        );
        let (e1x, e1y) = self.snap_point(
            x - (dx * (2.5 + flip) * 0.1 + dy * leg_scale) * size,
            y - (dy * (2.5 + flip) * 0.1 - dx * leg_scale) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(s1x, s1y);
        self.ctx.line_to(e1x, e1y);
        self.ctx.stroke();

        let (s2x, s2y) = self.snap_point(
            x + (dx * (2.5 - flip) * 0.1 - dy * leg_scale) * size,
            y + (dy * (2.5 - flip) * 0.1 + dx * leg_scale) * size,
        );
        let (e2x, e2y) = self.snap_point(
            x - (dx * (2.5 - flip) * 0.1 - dy * leg_scale) * size,
            y - (dy * (2.5 - flip) * 0.1 + dx * leg_scale) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(s2x, s2y);
        self.ctx.line_to(e2x, e2y);
        self.ctx.stroke();

        // Body fill (black) + outline (red), like DRAW_COLORS=0x41
        self.ctx.set_fill_style_str(COLOR_BG);
        self.draw_circle_filled(x, y, size);
        self.ctx.set_fill_style_str(COLOR_4);
        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * CREATURE_SCALE).max(1.0));
        self.draw_circle_outline(x, y, size);

        // Fangs on top so they remain visible
        let (f1x, f1y) = self.snap_point(
            x + (dx * 0.50 - dy * 0.1) * size,
            y + (dy * 0.50 + dx * 0.1) * size,
        );
        let (f2x, f2y) = self.snap_point(
            x + (dx * 0.65 - dy * 0.2) * size,
            y + (dy * 0.65 + dx * 0.2) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(f1x, f1y);
        self.ctx.line_to(f2x, f2y);
        self.ctx.stroke();

        let (f3x, f3y) = self.snap_point(
            x + (dx * 0.50 + dy * 0.1) * size,
            y + (dy * 0.50 - dx * 0.1) * size,
        );
        let (f4x, f4y) = self.snap_point(
            x + (dx * 0.65 + dy * 0.2) * size,
            y + (dy * 0.65 - dx * 0.2) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(f3x, f3y);
        self.ctx.line_to(f4x, f4y);
        self.ctx.stroke();
    }

    fn render_snake_snapshot(&self, id: usize, pos: Vec2, dir: Vec2, size: f32, frame_count: u32, camera: &Camera) {
        let screen_pos = camera.world_to_screen(pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let size = size as f64 * CREATURE_SCALE;
        self.ctx.set_global_alpha(1.0);
        let flip: f64 = if (frame_count / 5 + id as u32) % 2 == 0 { 1.0 } else { -1.0 };
        let (dx, dy) = if dir.x == 0.0 && dir.y == 0.0 {
            (0.0, 1.0)
        } else {
            (dir.x as f64, dir.y as f64)
        };

        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * CREATURE_SCALE).max(1.0));

        // Legs first so body fill masks inner parts
        let leg_scale = 1.15;
        let (s1x, s1y) = self.snap_point(
            x + (dx * (2.5 + flip) * 0.1 + dy * leg_scale) * size,
            y + (dy * (2.5 + flip) * 0.1 - dx * leg_scale) * size,
        );
        let (e1x, e1y) = self.snap_point(
            x - (dx * (2.5 + flip) * 0.1 + dy * leg_scale) * size,
            y - (dy * (2.5 + flip) * 0.1 - dx * leg_scale) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(s1x, s1y);
        self.ctx.line_to(e1x, e1y);
        self.ctx.stroke();

        let (s2x, s2y) = self.snap_point(
            x + (dx * (2.5 - flip) * 0.1 - dy * leg_scale) * size,
            y + (dy * (2.5 - flip) * 0.1 + dx * leg_scale) * size,
        );
        let (e2x, e2y) = self.snap_point(
            x - (dx * (2.5 - flip) * 0.1 - dy * leg_scale) * size,
            y - (dy * (2.5 - flip) * 0.1 + dx * leg_scale) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(s2x, s2y);
        self.ctx.line_to(e2x, e2y);
        self.ctx.stroke();

        // Body fill (black) + outline (red)
        self.ctx.set_fill_style_str(COLOR_BG);
        self.draw_circle_filled(x, y, size);
        self.ctx.set_fill_style_str(COLOR_4);
        self.ctx.set_stroke_style_str(COLOR_4);
        self.ctx.set_line_width((1.0 * CREATURE_SCALE).max(1.0));
        self.draw_circle_outline(x, y, size);

        // Fangs on top so they remain visible
        let (f1x, f1y) = self.snap_point(
            x + (dx * 0.50 - dy * 0.1) * size,
            y + (dy * 0.50 + dx * 0.1) * size,
        );
        let (f2x, f2y) = self.snap_point(
            x + (dx * 0.65 - dy * 0.2) * size,
            y + (dy * 0.65 + dx * 0.2) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(f1x, f1y);
        self.ctx.line_to(f2x, f2y);
        self.ctx.stroke();

        let (f3x, f3y) = self.snap_point(
            x + (dx * 0.50 + dy * 0.1) * size,
            y + (dy * 0.50 - dx * 0.1) * size,
        );
        let (f4x, f4y) = self.snap_point(
            x + (dx * 0.65 + dy * 0.2) * size,
            y + (dy * 0.65 - dx * 0.2) * size,
        );
        self.ctx.begin_path();
        self.ctx.move_to(f3x, f3y);
        self.ctx.line_to(f4x, f4y);
        self.ctx.stroke();
    }

    fn render_projectile(&self, projectile: &Projectile, camera: &Camera) {
        let screen_pos = camera.world_to_screen(projectile.pos);
        // Original: hostile = 0x22 (color 2), reflected = 0x21 (color 2 fill, color 1 outline)
        // In practice: hostile projectiles are blue, reflected are lighter
        let color = if projectile.hostile { COLOR_3 } else { COLOR_2 };
        self.ctx.set_fill_style_str(color);
        self.fill_circle(screen_pos.x as f64, screen_pos.y as f64, 1.5 * CREATURE_SCALE);
    }

    fn render_wisp(&self, wisp: &Wisp, frame_count: u32, camera: &Camera) {
        self.render_wisp_snapshot(wisp.id, wisp.pos, wisp.dir, frame_count, camera);
    }

    fn render_wisp_snapshot(&self, id: usize, pos: Vec2, dir: Vec2, frame_count: u32, camera: &Camera) {
        let screen_pos = camera.world_to_screen(pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let scale = CREATURE_SCALE;
        let pulse = ((frame_count / 6 + id as u32) % 10) as f64 / 10.0;

        self.ctx.set_fill_style_str(COLOR_LIGHT);
        self.ctx.set_global_alpha(0.6);
        self.fill_circle(x, y, 3.5 * scale);
        self.ctx.set_global_alpha(1.0);

        self.ctx.set_fill_style_str(COLOR_ACCENT1);
        let orbit = 4.0 * scale;
        let ox = x + (dir.x as f64 * orbit) * (0.6 + pulse);
        let oy = y + (dir.y as f64 * orbit) * (0.6 + pulse);
        self.fill_circle(ox, oy, 1.2 * scale);
    }

    fn render_guardian(&self, guardian: &crate::entities::Guardian, frame_count: u32, camera: &Camera) {
        let pos = guardian.pos;
        let dir = guardian.dir;
        let screen_pos = camera.world_to_screen(pos);
        let (x, y) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let scale = CREATURE_SCALE;
        let pulse = ((frame_count as f64 * 0.07).sin() * 0.3 + 0.7) as f64;

        let (_dx, _dy) = if dir.x == 0.0 && dir.y == 0.0 {
            (0.0, 1.0)
        } else {
            (dir.x as f64, dir.y as f64)
        };

        let body_radius = 7.5 * scale;
        self.ctx.set_fill_style_str("#2a3b44");
        self.fill_circle(x, y, body_radius as f64);

        self.ctx.set_fill_style_str("#546a74");
        self.fill_circle(x - 2.0 * scale as f64, y - 2.0 * scale as f64, (body_radius * 0.6) as f64);

        let tentacle_width = 2.2 * scale as f64;
        self.ctx.set_stroke_style_str("#7fa3b3");
        self.ctx.set_line_width(tentacle_width.max(1.0));

        for tentacle in guardian.tentacle_paths() {
            self.draw_tentacle_path(tentacle, camera, pulse);
        }

        if guardian.strike_active() {
            self.ctx.set_global_alpha(0.85);
            self.ctx.set_fill_style_str("#ff7d6b");
            for target in guardian.strike_points().iter() {
                let strike_screen = camera.world_to_screen(*target);
                let (sx, sy) = self.snap_point(strike_screen.x as f64, strike_screen.y as f64);
                self.fill_circle(sx, sy, (4.8 * scale) as f64);
            }
            let strike_screen = camera.world_to_screen(guardian.strike_pos());
            let (sx, sy) = self.snap_point(strike_screen.x as f64, strike_screen.y as f64);
            self.fill_circle(sx, sy, (5.5 * scale) as f64);
            self.ctx.set_global_alpha(1.0);
        }
    }

    fn draw_tentacle_path(&self, tentacle: &crate::entities::Tentacle, camera: &Camera, pulse: f64) {
        let mut first = true;
        for (idx, joint) in tentacle.joints.iter().enumerate() {
            let screen = camera.world_to_screen(*joint);
            let (x, y) = self.snap_point(screen.x as f64, screen.y as f64);
            if first {
                self.ctx.begin_path();
                self.ctx.move_to(x, y);
                first = false;
            } else {
                let jitter = (pulse * (idx as f64 + tentacle.mode as f64)).sin() * 1.2;
                self.ctx.line_to(x + jitter, y + jitter);
            }
        }
        self.ctx.stroke();
    }

    fn render_player(&self, player: &Player, camera: &Camera) {
        let screen_pos = camera.world_to_screen(player.pos);
        let (cx, cy) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let render_cx = self.world_to_render_pixel(screen_pos.x as f64) as f64;
        let render_cy = self.world_to_render_pixel(screen_pos.y as f64) as f64;
        let scale = CREATURE_SCALE;

        if player.is_shielded() {
            self.ctx.save();
            let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            self.ctx.set_global_alpha(0.45);
            self.ctx.set_stroke_style_str(COLOR_LIGHT);
            let radius = self.world_to_render_size(14.0 * scale) as f64;
            self.draw_circle_outline(render_cx, render_cy, (radius / 2.0).ceil());
            self.ctx.set_global_alpha(1.0);
            self.ctx.restore();
        }

        // Phase effect - ghostly trail when phasing
        if player.is_phasing() {
            self.ctx.set_fill_style_str(COLOR_ACCENT1);
            self.ctx.set_global_alpha(0.3);
            for i in 1..=3 {
                let offset = i as f64 * 5.0 * scale;
                let (ghost_x, ghost_y) = self.snap_point(
                    cx - player.phase_dir.x as f64 * offset,
                    cy - player.phase_dir.y as f64 * offset,
                );
                let ghost_alpha = 0.3 - (i as f64 * 0.08);
                self.ctx.set_global_alpha(ghost_alpha);
                self.fill_circle(ghost_x, ghost_y, (4.0 - i as f64 * 0.5) * scale);
            }
            self.ctx.set_global_alpha(1.0);
        }

        // Attack effect (original: light circle at look_dir * 5, size 17, plus expanding ring from player)
        if player.is_attacking() {
            self.ctx.save();
            let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            self.ctx.set_fill_style_str(COLOR_LIGHT);
            let attack_x = render_cx + player.look_dir.x as f64 * 5.0 * scale * self.scale_x;
            let attack_y = render_cy + player.look_dir.y as f64 * 5.0 * scale * self.scale_y;
            let attack_diameter = self.world_to_render_size(17.0 * scale) as f64;
            let attack_radius = (attack_diameter / 2.0).ceil();
            self.draw_circle_filled(attack_x, attack_y, attack_radius);
            self.ctx.set_fill_style_str(COLOR_BG);
            let ring_size = (15 - player.attack_timer) as f64;
            if ring_size > 0.0 {
                let ring_diameter = self.world_to_render_size(ring_size * scale) as f64;
                let ring_radius = (ring_diameter / 2.0).ceil();
                self.draw_circle_filled(render_cx, render_cy, ring_radius);
            }
            self.ctx.restore();
        }

        // Player body (slime) - original uses magenta for normal, blue for phasing
        let body_color = if player.is_phasing() { COLOR_ACCENT1 } else { COLOR_ACCENT2 };
        self.ctx.set_fill_style_str(body_color);

        // Phase effect: make player semi-transparent, original feels ghostly
        if player.is_phasing() {
            self.ctx.set_global_alpha(0.6);
        }

        self.ctx.save();
        let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

        // Tail effect (original: single blob at move_dir * -4, size 5).
        let tail_dir = player.move_dir;
        let tail_x = render_cx - tail_dir.x as f64 * 4.0 * scale * self.scale_x;
        let tail_y = render_cy - tail_dir.y as f64 * 4.0 * scale * self.scale_y;
        let tail_diameter = self.world_to_render_size(5.0 * scale) as f64;
        let tail_radius = (tail_diameter / 2.0).ceil();
        if player.is_phasing() {
            let body_diameter = self.world_to_render_size(9.0 * scale) as f64;
            let body_radius = (body_diameter / 2.0).ceil();
            self.draw_circle_outline_masked(tail_x, tail_y, tail_radius, render_cx, render_cy, body_radius);
        } else {
            self.draw_circle_filled(tail_x, tail_y, tail_radius);
        }

        // Square body center (original: 7x7 rect at x-3, y-3)
        let rect_size = self.world_to_render_size(7.0 * scale) as f64;
        let rect_half = (rect_size / 2.0).floor();
        if !player.is_phasing() {
            self.draw_rect_filled(render_cx - rect_half, render_cy - rect_half, rect_size, rect_size);
        }

        // Main body (original: size 9)
        let body_diameter = self.world_to_render_size(9.0 * scale) as f64;
        let body_radius = (body_diameter / 2.0).ceil();
        if player.is_phasing() {
            self.draw_circle_outline(render_cx, render_cy, body_radius);
        } else {
            self.draw_circle_filled(render_cx, render_cy, body_radius);
        }

        // Eyes (original: at look_dir * 3 +/- perpendicular * 1.5, single pixels)
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        let eye1_x = render_cx + (player.look_dir.x as f64 * 3.0 + player.look_dir.y as f64 * 1.5) * scale * self.scale_x;
        let eye1_y = render_cy + (player.look_dir.y as f64 * 3.0 - player.look_dir.x as f64 * 1.5) * scale * self.scale_y;
        let eye2_x = render_cx + (player.look_dir.x as f64 * 3.0 - player.look_dir.y as f64 * 1.5) * scale * self.scale_x;
        let eye2_y = render_cy + (player.look_dir.y as f64 * 3.0 + player.look_dir.x as f64 * 1.5) * scale * self.scale_y;
        let eye_size = self.world_to_render_size(1.0 * scale) as f64;
        self.draw_rect_filled(eye1_x.round(), eye1_y.round(), eye_size, eye_size);
        self.draw_rect_filled(eye2_x.round(), eye2_y.round(), eye_size, eye_size);

        // Shield (blocking) - original has two lines
        if player.blocking {
            self.ctx.set_fill_style_str(COLOR_LIGHT);
            let s1x = render_cx + (player.look_dir.x as f64 * 5.0 - player.look_dir.y as f64 * 7.0) * scale * self.scale_x;
            let s1y = render_cy + (player.look_dir.y as f64 * 5.0 + player.look_dir.x as f64 * 7.0) * scale * self.scale_y;
            let e1x = render_cx + (player.look_dir.x as f64 * 5.0 + player.look_dir.y as f64 * 7.0) * scale * self.scale_x;
            let e1y = render_cy + (player.look_dir.y as f64 * 5.0 - player.look_dir.x as f64 * 7.0) * scale * self.scale_y;
            self.draw_line_scaled(s1x.round(), s1y.round(), e1x.round(), e1y.round(), scale);

            let s2x = render_cx + (player.look_dir.x as f64 * 6.0 - player.look_dir.y as f64 * 4.0) * scale * self.scale_x;
            let s2y = render_cy + (player.look_dir.y as f64 * 6.0 + player.look_dir.x as f64 * 4.0) * scale * self.scale_y;
            let e2x = render_cx + (player.look_dir.x as f64 * 6.0 + player.look_dir.y as f64 * 4.0) * scale * self.scale_x;
            let e2y = render_cy + (player.look_dir.y as f64 * 6.0 - player.look_dir.x as f64 * 4.0) * scale * self.scale_y;
            self.draw_line_scaled(s2x.round(), s2y.round(), e2x.round(), e2y.round(), scale);
        }

        self.ctx.restore();

        // Reset alpha after any phasing
        if player.is_phasing() {
            self.ctx.set_global_alpha(1.0);
        }
    }

    fn render_remote_player(&self, remote: &RemotePlayer, camera: &Camera) {
        let screen_pos = camera.world_to_screen(remote.pos);
        let (cx, cy) = self.snap_point(screen_pos.x as f64, screen_pos.y as f64);
        let render_cx = self.world_to_render_pixel(screen_pos.x as f64) as f64;
        let render_cy = self.world_to_render_pixel(screen_pos.y as f64) as f64;
        let scale = CREATURE_SCALE;

        if remote.shielded {
            self.ctx.save();
            let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            self.ctx.set_global_alpha(0.45);
            self.ctx.set_stroke_style_str(COLOR_LIGHT);
            let radius = self.world_to_render_size(14.0 * scale) as f64;
            self.draw_circle_outline(render_cx, render_cy, (radius / 2.0).ceil());
            self.ctx.set_global_alpha(1.0);
            self.ctx.restore();
        }

        // Phase effect - ghostly trail when phasing
        if remote.phasing {
            self.ctx.set_fill_style_str(COLOR_ACCENT1);
            for i in 1..=3 {
                let offset = i as f64 * 5.0 * scale;
                let (ghost_x, ghost_y) = self.snap_point(
                    cx - remote.move_dir.x as f64 * offset,
                    cy - remote.move_dir.y as f64 * offset,
                );
                let ghost_alpha = 0.3 - (i as f64 * 0.08);
                self.ctx.set_global_alpha(ghost_alpha);
                self.fill_circle(ghost_x, ghost_y, (4.0 - i as f64 * 0.5) * scale);
            }
            self.ctx.set_global_alpha(1.0);
        }

        // Attack effect (similar to player but green)
        if remote.attacking {
            self.ctx.save();
            let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            self.ctx.set_fill_style_str(COLOR_REMOTE_PLAYER);
            let attack_x = render_cx + remote.look_dir.x as f64 * 5.0 * scale * self.scale_x;
            let attack_y = render_cy + remote.look_dir.y as f64 * 5.0 * scale * self.scale_y;
            let attack_diameter = self.world_to_render_size(17.0 * scale) as f64;
            let attack_radius = (attack_diameter / 2.0).ceil();
            self.draw_circle_filled(attack_x, attack_y, attack_radius);
            self.ctx.restore();
        }

        // Remote player body - green tinted
        let body_color = if remote.phasing { COLOR_ACCENT1 } else { COLOR_REMOTE_PLAYER };
        self.ctx.set_fill_style_str(body_color);

        // Phase effect: make remote player semi-transparent
        if remote.phasing {
            self.ctx.set_global_alpha(0.6);
        }

        self.ctx.save();
        let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

        // Tail effect (single blob), always based on move_dir like original
        let tail_dir = remote.move_dir;
        let tail_x = render_cx - tail_dir.x as f64 * 4.0 * scale * self.scale_x;
        let tail_y = render_cy - tail_dir.y as f64 * 4.0 * scale * self.scale_y;
        let tail_diameter = self.world_to_render_size(5.0 * scale) as f64;
        let tail_radius = (tail_diameter / 2.0).ceil();
        if remote.phasing {
            let body_diameter = self.world_to_render_size(9.0 * scale) as f64;
            let body_radius = (body_diameter / 2.0).ceil();
            self.draw_circle_outline_masked(tail_x, tail_y, tail_radius, render_cx, render_cy, body_radius);
        } else {
            self.draw_circle_filled(tail_x, tail_y, tail_radius);
        }

        // Square body center
        let rect_size = self.world_to_render_size(7.0 * scale) as f64;
        let rect_half = (rect_size / 2.0).floor();
        if !remote.phasing {
            self.draw_rect_filled(render_cx - rect_half, render_cy - rect_half, rect_size, rect_size);
        }

        // Main body
        let body_diameter = self.world_to_render_size(9.0 * scale) as f64;
        let body_radius = (body_diameter / 2.0).ceil();
        if remote.phasing {
            self.draw_circle_outline(render_cx, render_cy, body_radius);
        } else {
            self.draw_circle_filled(render_cx, render_cy, body_radius);
        }

        // Eyes (single pixels)
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        let eye1_x = render_cx + (remote.look_dir.x as f64 * 3.0 + remote.look_dir.y as f64 * 1.5) * scale * self.scale_x;
        let eye1_y = render_cy + (remote.look_dir.y as f64 * 3.0 - remote.look_dir.x as f64 * 1.5) * scale * self.scale_y;
        let eye2_x = render_cx + (remote.look_dir.x as f64 * 3.0 - remote.look_dir.y as f64 * 1.5) * scale * self.scale_x;
        let eye2_y = render_cy + (remote.look_dir.y as f64 * 3.0 + remote.look_dir.x as f64 * 1.5) * scale * self.scale_y;
        let eye_size = self.world_to_render_size(1.0 * scale) as f64;
        self.draw_rect_filled(eye1_x.round(), eye1_y.round(), eye_size, eye_size);
        self.draw_rect_filled(eye2_x.round(), eye2_y.round(), eye_size, eye_size);

        // Shield (blocking)
        if remote.blocking {
            self.ctx.set_fill_style_str(COLOR_LIGHT);
            let s1x = render_cx + (remote.look_dir.x as f64 * 5.0 - remote.look_dir.y as f64 * 7.0) * scale * self.scale_x;
            let s1y = render_cy + (remote.look_dir.y as f64 * 5.0 + remote.look_dir.x as f64 * 7.0) * scale * self.scale_y;
            let e1x = render_cx + (remote.look_dir.x as f64 * 5.0 + remote.look_dir.y as f64 * 7.0) * scale * self.scale_x;
            let e1y = render_cy + (remote.look_dir.y as f64 * 5.0 - remote.look_dir.x as f64 * 7.0) * scale * self.scale_y;
            self.draw_line_scaled(s1x.round(), s1y.round(), e1x.round(), e1y.round(), scale);

            let s2x = render_cx + (remote.look_dir.x as f64 * 6.0 - remote.look_dir.y as f64 * 4.0) * scale * self.scale_x;
            let s2y = render_cy + (remote.look_dir.y as f64 * 6.0 + remote.look_dir.x as f64 * 4.0) * scale * self.scale_y;
            let e2x = render_cx + (remote.look_dir.x as f64 * 6.0 + remote.look_dir.y as f64 * 4.0) * scale * self.scale_x;
            let e2y = render_cy + (remote.look_dir.y as f64 * 6.0 - remote.look_dir.x as f64 * 4.0) * scale * self.scale_y;
            self.draw_line_scaled(s2x.round(), s2y.round(), e2x.round(), e2y.round(), scale);
        }

        self.ctx.restore();

        if remote.phasing {
            self.ctx.set_global_alpha(1.0);
        }

        // Draw player name above (overlay uses full-res; keep this for fallback)
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        self.ctx.set_font("10px monospace");
        self.ctx.set_text_align("center");
        let _ = self.ctx.fill_text(&remote.name, cx, cy - 18.0);
    }

    fn render_network_status(&self, network: &NetworkSession) {
        self.ctx.set_font("12px monospace");
        self.ctx.set_text_align("right");

        let status_text = match &network.state {
            NetworkState::Disconnected => "Offline".to_string(),
            NetworkState::Connecting => "Connecting...".to_string(),
            NetworkState::WaitingForPeers => format!("Room: {} (waiting)", network.room_code),
            NetworkState::Connected => format!("Room: {} ({} players)", network.room_code, network.peer_count() + 1),
            NetworkState::Error(e) => format!("Error: {}", e),
        };

        let color = match network.state {
            NetworkState::Disconnected => "#666666",
            NetworkState::Connecting => "#ffff00",
            NetworkState::WaitingForPeers => "#00ff00",
            NetworkState::Connected => "#00ff00",
            NetworkState::Error(_) => "#ff0000",
        };

        self.ctx.set_fill_style_str(color);
        let _ = self.ctx.fill_text(&status_text, (self.width - 10) as f64, 25.0);
    }

    fn render_title_overlay(&self, game: &Game, network: &NetworkSession) {
        let center_x = (self.width / 2) as f64;

        self.display_ctx.set_global_alpha(1.0);

        // Title text
        self.display_ctx.set_fill_style_str(COLOR_ACCENT2);
        self.display_ctx.set_font("bold 36px monospace");
        self.display_ctx.set_text_align("center");
        let _ = self.display_ctx.fill_text("SLIME ARMIES", center_x, 60.0);

        // Player name input
        let name_y = 220.0;
        self.display_ctx.set_font("14px monospace");
        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_text_align("right");
        let _ = self.display_ctx.fill_text("Name:", center_x - 100.0, name_y);

        let is_name_active = game.text_input_active && game.active_input_field == 0;
        self.render_text_input_on(
            &self.display_ctx,
            center_x - 90.0,
            name_y - 14.0,
            180.0,
            &game.player_name,
            is_name_active,
            game.frame_count,
        );

        // Room code input
        let code_y = 260.0;
        self.display_ctx.set_text_align("right");
        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        let _ = self.display_ctx.fill_text("Room:", center_x - 100.0, code_y);

        let is_code_active = game.text_input_active && game.active_input_field == 1;
        self.render_text_input_on(
            &self.display_ctx,
            center_x - 90.0,
            code_y - 14.0,
            100.0,
            &game.room_code_input,
            is_code_active,
            game.frame_count,
        );

        // Join button next to room code
        let join_x = center_x + 20.0;
        let join_y = code_y - 14.0;
        let join_active = is_code_active;
        let join_bg = if join_active { "#223344" } else { "#111122" };
        self.display_ctx.set_fill_style_str(join_bg);
        self.display_ctx.fill_rect(join_x, join_y, 70.0, 22.0);
        self.display_ctx.set_fill_style_str(if join_active { COLOR_ACCENT1 } else { COLOR_LIGHT });
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("center");
        let _ = self.display_ctx.fill_text("JOIN", join_x + 35.0, code_y);

        // Menu options
        let menu_y = 320.0;
        let menu_spacing = 40.0;

        self.render_menu_item_on(
            &self.display_ctx,
            center_x,
            menu_y,
            "SOLO PLAY",
            game.menu_selection == MenuSelection::Play,
            game.frame_count,
        );
        self.render_menu_item_on(
            &self.display_ctx,
            center_x,
            menu_y + menu_spacing,
            "CREATE ROOM",
            game.menu_selection == MenuSelection::CreateRoom,
            game.frame_count,
        );

        self.render_network_status_on(&self.display_ctx, network);

        // Instructions
        self.display_ctx.set_fill_style_str("#666666");
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("center");
        let base_y = (self.height - 70) as f64;
        let _ = self.display_ctx.fill_text(
            "Click name/room to edit | Enter joins room",
            center_x,
            base_y,
        );
        let _ = self.display_ctx.fill_text(
            "Arrow keys navigate | SPACE select",
            center_x,
            base_y + 18.0,
        );
        let _ = self.display_ctx.fill_text(
            "In-game: WASD/Arrows: Move | Z/Space: Attack/Block | X/Shift: Phase | M: Map",
            center_x,
            (self.height - 40) as f64,
        );
    }

    fn render_game_overlay(&self, game: &Game, network: &NetworkSession) {
        let camera = &game.camera;

        self.display_ctx.set_global_alpha(1.0);

        // Player names
        if game.player.alive {
            let screen_pos = camera.world_to_screen(game.player.pos);
            self.display_ctx.set_fill_style_str(COLOR_LIGHT);
            self.display_ctx.set_font("10px monospace");
            self.display_ctx.set_text_align("center");
            let _ = self
                .display_ctx
                .fill_text(&network.local_player_name, screen_pos.x as f64, screen_pos.y as f64 - 18.0);
        }

        for remote in network.remote_players.values() {
            if remote.alive {
                let screen_pos = camera.world_to_screen(remote.pos);
                self.display_ctx.set_fill_style_str(COLOR_LIGHT);
                self.display_ctx.set_font("10px monospace");
                self.display_ctx.set_text_align("center");
                let _ = self.display_ctx.fill_text(&remote.name, screen_pos.x as f64, screen_pos.y as f64 - 18.0);
            }
        }

        // HUD
        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_font("14px monospace");
        self.display_ctx.set_text_align("left");
        let _ = self
            .display_ctx
            .fill_text(&format!("Wave: {}", game.wave), 10.0, 25.0);
        let _ = self
            .display_ctx
            .fill_text(&format!("Kills: {}", game.kills), 10.0, 45.0);
        let display_x = game.player.pos.x / 1000.0;
        let display_y = game.player.pos.y / 1000.0;
        let _ = self.display_ctx.fill_text(
            &format!("Pos: ({:.1}, {:.1})", display_x, display_y),
            10.0,
            65.0,
        );
        if game.shrine_badge_unlocked {
            let _ = self.display_ctx.fill_text("Badge: Shrinefinder", 10.0, 85.0);
        }
        if network.room_code.is_empty() {
            let stats = PlayerStats {
                kills: game.kills,
                spider_kills: game.spider_kills,
                cannon_kills: game.cannon_kills,
                snake_kills: game.snake_kills,
                wisp_kills: game.wisp_kills,
                attack_attempts: game.attack_attempts,
                attack_hits: game.attack_hits,
                deaths: game.deaths,
                time_played_frames: game.frame_count.saturating_sub(game.start_frame),
            };
            let score = network.score_for_stats(&stats);
            let _ = self
                .display_ctx
                .fill_text(&format!("Score: {}", score), 10.0, if game.shrine_badge_unlocked { 105.0 } else { 85.0 });
        }
        let map_size = 120.0;
        let map_padding = 10.0;
        let map_left = (self.width as f64) - map_size - map_padding;
        let portrait = game.viewport_height > game.viewport_width;
        let map_top = if game.mobile_mode || portrait { 130.0 } else { (self.height as f64) - map_size - map_padding };
        if !game.mobile_mode {
            self.display_ctx.set_font("12px monospace");
            self.display_ctx.set_text_align("left");
            let _ = self.display_ctx.fill_text("M: Map", map_left, map_top - 6.0);
            let _ = self.display_ctx.fill_text("B: Bind Abilities", map_left, map_top - 22.0);
            let _ = self.display_ctx.fill_text("4: Payments", map_left, map_top - 36.0);
            let _ = self.display_ctx.fill_text("F3: Net Debug", map_left, map_top - 50.0);
        }

        if !network.room_code.is_empty() {
            self.render_team_stats(game, network);
        }
        if game.net_debug_overlay {
            self.render_network_debug_overlay(network);
        }
        self.render_minimap(game, network);
        if game.map_open {
            self.render_map_overlay(game, network);
        }
        if !game.map_open {
            self.render_chat_overlay(game);
        }
        if game.mobile_mode {
            self.render_mobile_controls(game);
        }

        if !game.map_open {
            self.render_ability_bar(game);
        }

        self.render_network_status_on(&self.display_ctx, network);
    }

    fn render_ability_bar(&self, game: &Game) {
        let entries = game.ability_bar_entries();
        if entries.is_empty() {
            return;
        }

        self.display_ctx.set_font("10px monospace");
        self.display_ctx.set_text_align("center");

        for (idx, entry) in entries.iter().enumerate() {
            let color = if entry.ready { COLOR_LIGHT } else { "#556" };
            self.display_ctx.set_stroke_style_str(color);
            let _ = self.display_ctx.stroke_rect(entry.x, entry.y, entry.w, entry.h);

            self.display_ctx.set_fill_style_str(color);
            let label_y = entry.y + entry.h * 0.55;
            let _ = self.display_ctx.fill_text(&entry.label, entry.x + entry.w * 0.5, label_y);

            if !entry.key.is_empty() {
                let key = entry.key.trim_start_matches("Key");
                let key_y = entry.y + entry.h + 10.0;
                let _ = self.display_ctx.fill_text(key, entry.x + entry.w * 0.5, key_y);
            }

            if game.ability_bind_open && idx == game.ability_bind_selected() {
                self.display_ctx.set_stroke_style_str(COLOR_ACCENT2);
                let _ = self.display_ctx.stroke_rect(entry.x - 2.0, entry.y - 2.0, entry.w + 4.0, entry.h + 4.0);
            }
        }

        if game.ability_bind_open {
            self.display_ctx.set_fill_style_str(COLOR_LIGHT);
            self.display_ctx.set_font("12px monospace");
            let msg = if game.ability_bind_waiting() {
                "Press a key to bind (Esc to cancel)"
            } else {
                "Bind: arrows to select, Enter to rebind, Esc to close"
            };
            let _ = self.display_ctx.fill_text(msg, (self.width as f64) * 0.5, (self.height as f64) - 10.0);
        }
    }


    fn render_gameover_overlay(&self, game: &Game, network: &NetworkSession) {
        self.display_ctx.set_global_alpha(1.0);

        if game.map_open {
            self.render_map_overlay(game, network);
            return;
        }

        self.display_ctx.set_fill_style_str(COLOR_ACCENT2);
        self.display_ctx.set_font("bold 48px monospace");
        self.display_ctx.set_text_align("center");
        let _ = self.display_ctx.fill_text(
            "GAME OVER",
            (self.width / 2) as f64,
            (self.height / 2 - 50) as f64,
        );

        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_font("24px monospace");
        let _ = self.display_ctx.fill_text(
            &format!("Wave: {}", game.wave),
            (self.width / 2) as f64,
            (self.height / 2 + 20) as f64,
        );
        let _ = self.display_ctx.fill_text(
            &format!("Kills: {}", game.kills),
            (self.width / 2) as f64,
            (self.height / 2 + 55) as f64,
        );

        let time = (game.end_frame - game.start_frame) / 60;
        let _ = self.display_ctx.fill_text(
            &format!("Time: {}s", time),
            (self.width / 2) as f64,
            (self.height / 2 + 90) as f64,
        );

        self.display_ctx.set_font("16px monospace");
        let help_text = if game.mobile_mode {
            "Tap screen to open map"
        } else {
            "Press Z or SPACE to open map"
        };
        let _ = self.display_ctx.fill_text(
            help_text,
            (self.width / 2) as f64,
            (self.height - 80) as f64,
        );
    }

    fn render_team_stats(&self, game: &Game, network: &NetworkSession) {
        let entries = self.collect_player_entries(game, network);
        let mut top_entries = entries.clone();
        top_entries.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| b.kills.cmp(&a.kills)));

        let right_x = (self.width - 10) as f64;
        let mut y = 45.0;

        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("right");

        let _ = self.display_ctx.fill_text("Top Players", right_x, y);
        y += 16.0;

        for (idx, entry) in top_entries.iter().take(3).enumerate() {
            let _ = self.display_ctx.fill_text(
                &format!(
                    "{}. {} S:{} K:{} D:{}",
                    idx + 1,
                    entry.name,
                    entry.score,
                    entry.kills,
                    entry.deaths,
                ),
                right_x,
                y,
            );
            y += 14.0;
        }

        self.display_ctx.set_fill_style_str("#666666");
        let _ = self.display_ctx.fill_text("P: Players", right_x, y + 4.0);

        if game.player_list_open && !game.map_open {
            self.render_player_list_overlay(game, network, &entries);
        }
    }

    fn render_network_debug_overlay(&self, network: &NetworkSession) {
        let telemetry = &network.relay_telemetry;
        let queue_depth = network.relay_queue_depth();
        let congestion = network.relay_congestion_level();
        let role = if network.is_host { "ROOT" } else { "NODE" };

        let left = 10.0;
        let top = 110.0;
        let width = 290.0;
        let height = 96.0;

        self.display_ctx.set_fill_style_str("rgba(0,0,0,0.55)");
        self.display_ctx.fill_rect(left, top, width, height);
        self.display_ctx.set_stroke_style_str("#445566");
        self.display_ctx.set_line_width(1.0);
        self.display_ctx.stroke_rect(left, top, width, height);

        self.display_ctx.set_fill_style_str("#d7e6e6");
        self.display_ctx.set_font("11px monospace");
        self.display_ctx.set_text_align("left");
        let _ = self.display_ctx.fill_text(
            &format!(
                "Net {} {} q:{} lvl:{} conn:{} known:{} links:{}",
                role,
                if network.discovery_attached() { "DISC" } else { "OVR" },
                queue_depth,
                congestion,
                network.peer_count() + 1,
                network.known_peer_count(),
                network.desired_peer_count()
            ),
            left + 8.0,
            top + 16.0,
        );
        let _ = self.display_ctx.fill_text(
            &format!(
                "rx:{} up:{} dn:{} bc:{}",
                telemetry.recv_messages, telemetry.sent_upstream, telemetry.sent_downstream, telemetry.sent_broadcast
            ),
            left + 8.0,
            top + 34.0,
        );
        let _ = self.display_ctx.fill_text(
            &format!(
                "drop:{} qdrop:{} maxq:{} sw:{}",
                telemetry.dropped_messages,
                telemetry.dropped_queue_entries,
                telemetry.max_queue_depth,
                telemetry.stale_parent_switches
            ),
            left + 8.0,
            top + 52.0,
        );
        let _ = self.display_ctx.fill_text(
            &format!(
                "root:{:?} supernodes:{} fanout:{}",
                network.supernode_id,
                network.supernode_set.len(),
                network.relay_fanout()
            ),
            left + 8.0,
            top + 70.0,
        );
        let _ = self.display_ctx.fill_text("F3 to hide", left + 8.0, top + 88.0);
    }

    fn render_player_list_overlay(&self, game: &Game, network: &NetworkSession, entries: &[PlayerEntry]) {
        let overlay_w = 480.0;
        let overlay_h = 360.0;
        let left = (self.width as f64 - overlay_w) / 2.0;
        let top = 70.0;

        self.display_ctx.set_fill_style_str("rgba(0,0,0,0.6)");
        self.display_ctx.fill_rect(0.0, 0.0, self.width as f64, self.height as f64);
        self.display_ctx.set_fill_style_str("#0d1214");
        self.display_ctx.fill_rect(left, top, overlay_w, overlay_h);
        self.display_ctx.set_stroke_style_str("#3a4a4a");
        self.display_ctx.set_line_width(2.0);
        self.display_ctx.stroke_rect(left, top, overlay_w, overlay_h);

        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_font("14px monospace");
        self.display_ctx.set_text_align("left");
        let _ = self.display_ctx.fill_text("Players", left + 12.0, top + 24.0);
        self.draw_overlay_close_button(left + overlay_w - 26.0, top + 8.0);
        let sort_label = match game.player_list_sort {
            0 => "score",
            1 => "name",
            2 => "kills",
            3 => "deaths",
            _ => "time",
        };
        let order_label = if game.player_list_sort_asc { "asc" } else { "desc" };
        let search = if game.player_list_search.is_empty() {
            "search: /".to_string()
        } else {
            format!("search: {}", game.player_list_search)
        };
        self.display_ctx.set_font("12px monospace");
        let _ = self.display_ctx.fill_text(
            &format!("Sort: {} ({})  {}", sort_label, order_label, search),
            left + 12.0,
            top + 40.0,
        );

        let row_height = 18.0;
        let rows_visible = 14;
        let max_scroll = entries.len().saturating_sub(rows_visible) as i32;
        let start = game.player_list_scroll.min(max_scroll).max(0) as usize;

        let header_y = top + 64.0;
        let name_x = left + 12.0;
        let score_x = left + 210.0;
        let kills_x = left + 270.0;
        let deaths_x = left + 320.0;
        let time_x = left + 370.0;

        self.display_ctx.set_fill_style_str("#aab3b3");
        let _ = self.display_ctx.fill_text("Name", name_x, header_y);
        let _ = self.display_ctx.fill_text("S", score_x, header_y);
        let _ = self.display_ctx.fill_text("K", kills_x, header_y);
        let _ = self.display_ctx.fill_text("D", deaths_x, header_y);
        let _ = self.display_ctx.fill_text("T", time_x, header_y);

        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        let mut y = header_y + row_height;
        for entry in entries.iter().skip(start).take(rows_visible) {
            let name = if entry.name.len() > 14 {
                format!("{}…", &entry.name[..13])
            } else {
                entry.name.clone()
            };
            let _ = self.display_ctx.fill_text(&name, name_x, y);
            self.display_ctx.set_text_align("right");
            let _ = self.display_ctx.fill_text(&entry.score.to_string(), score_x + 26.0, y);
            let _ = self.display_ctx.fill_text(&entry.kills.to_string(), kills_x + 20.0, y);
            let _ = self.display_ctx.fill_text(&entry.deaths.to_string(), deaths_x + 20.0, y);
            let _ = self.display_ctx.fill_text(&Self::format_time(entry.time_seconds), time_x + 46.0, y);
            self.display_ctx.set_text_align("left");
            y += row_height;
        }

        self.display_ctx.set_fill_style_str("#e6efef");
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("left");
        let _ = self.display_ctx.fill_text(
            "Players: Up/Down scroll | S sort | D order | / search | Esc clear | P close",
            left,
            top - 12.0,
        );
        self.render_network_status_on(&self.display_ctx, network);
    }

    fn draw_overlay_close_button(&self, x: f64, y: f64) {
        let size = 18.0;
        self.display_ctx.set_fill_style_str("#1b1f24");
        self.display_ctx.fill_rect(x, y, size, size);
        self.display_ctx.set_stroke_style_str("#5a6a6a");
        self.display_ctx.set_line_width(1.0);
        self.display_ctx.stroke_rect(x, y, size, size);
        self.display_ctx.set_fill_style_str("#e6efef");
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("center");
        let _ = self.display_ctx.fill_text("X", x + size / 2.0, y + 13.0);
    }

    fn render_chat_overlay(&self, game: &Game) {
        let log_lines: Vec<_> = game.chat_log.iter().rev().take(6).collect();
        if log_lines.is_empty() && !game.chat_open {
            return;
        }

        let line_height = 16.0;
        let padding = 6.0;
        let left = 10.0;
        let bottom = self.height as f64 - 10.0;
        let log_height = log_lines.len() as f64 * line_height;
        let helper_height = if game.chat_open { line_height } else { 0.0 };
        let input_height = if game.chat_open { line_height + 6.0 } else { 0.0 };
        let box_height = log_height + helper_height + input_height + padding * 2.0;
        let box_top = bottom - box_height;

        self.display_ctx.set_fill_style_str("rgba(0,0,0,0.45)");
        self.display_ctx.fill_rect(left - 4.0, box_top - 4.0, 360.0, box_height + 8.0);

        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("left");
        self.display_ctx.set_fill_style_str("#e6efef");

        let mut y = box_top + padding + line_height;
        for line in log_lines.iter().rev() {
            let text = format!("{}: {}", line.name, line.text);
            let _ = self.display_ctx.fill_text(&text, left, y);
            y += line_height;
        }

        if game.chat_open {
            let mut input = game.chat_input.clone();
            if input.len() > 72 {
                input = format!("...{}", &input[input.len().saturating_sub(69)..]);
            }
            let prompt = format!("> {}_", input);
            let helper_y = bottom - padding - input_height;
            self.display_ctx.set_fill_style_str("#9aa3a3");
            let _ = self
                .display_ctx
                .fill_text("Use /mute NAME to vote mute", left, helper_y);
            self.display_ctx.set_fill_style_str("#e6efef");
            let _ = self.display_ctx.fill_text(&prompt, left, bottom - padding);
        } else {
            if log_lines.is_empty() {
                self.display_ctx.set_fill_style_str("#9aa3a3");
                let _ = self.display_ctx.fill_text("C: Chat", left, bottom - padding);
            }
        }
    }

    fn render_mobile_controls(&self, game: &Game) {
        if game.map_open || game.player_list_open {
            return;
        }
        let width = self.width as f64;
        let height = self.height as f64;
        let stick_center = Vec2::new(90.0, height as f32 - 90.0);
        let stick_radius = 60.0;
        let action_radius = 36.0;
        let attack_center = Vec2::new(width as f32 - 80.0, height as f32 - 130.0);
        let phase_center = Vec2::new(width as f32 - 200.0, height as f32 - 60.0);
        let top_radius = 26.0;
        let chat_center = Vec2::new(width as f32 * 0.5, 32.0);
        let zoom_in_center = Vec2::new(width as f32 - 50.0, height as f32 - 220.0);
        let zoom_out_center = Vec2::new(width as f32 - 110.0, height as f32 - 220.0);

        // Joystick area - blue with transparency for better visibility
        self.display_ctx.set_fill_style_str("rgba(17,102,204,0.4)");
        self.display_ctx.begin_path();
        let _ = self.display_ctx.arc(stick_center.x as f64, stick_center.y as f64, stick_radius, 0.0, std::f64::consts::PI * 2.0);
        self.display_ctx.fill();

        // Attack and Phase buttons - blue with transparency for better visibility
        self.display_ctx.set_fill_style_str("rgba(17,102,204,0.5)");
        self.display_ctx.begin_path();
        let _ = self.display_ctx.arc(attack_center.x as f64, attack_center.y as f64, action_radius, 0.0, std::f64::consts::PI * 2.0);
        self.display_ctx.fill();
        self.display_ctx.begin_path();
        let _ = self.display_ctx.arc(phase_center.x as f64, phase_center.y as f64, action_radius, 0.0, std::f64::consts::PI * 2.0);
        self.display_ctx.fill();

        // Top buttons - blue with transparency
        self.display_ctx.set_fill_style_str("rgba(17,102,204,0.45)");
        self.display_ctx.begin_path();
        let _ = self.display_ctx.arc(chat_center.x as f64, chat_center.y as f64, top_radius, 0.0, std::f64::consts::PI * 2.0);
        self.display_ctx.fill();

        if game.map_open {
            for center in [zoom_in_center, zoom_out_center] {
                self.display_ctx.begin_path();
                let _ = self.display_ctx.arc(center.x as f64, center.y as f64, top_radius, 0.0, std::f64::consts::PI * 2.0);
                self.display_ctx.fill();
            }
        }

        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("center");
        self.display_ctx.set_fill_style_str("#dfe7e7");
        let _ = self.display_ctx.fill_text("ATT", attack_center.x as f64, attack_center.y as f64 + 4.0);
        let _ = self.display_ctx.fill_text("PH", phase_center.x as f64, phase_center.y as f64 + 4.0);
        let _ = self.display_ctx.fill_text("C", chat_center.x as f64, chat_center.y as f64 + 4.0);
        if game.map_open {
            let _ = self.display_ctx.fill_text("+", zoom_in_center.x as f64, zoom_in_center.y as f64 + 4.0);
            let _ = self.display_ctx.fill_text("-", zoom_out_center.x as f64, zoom_out_center.y as f64 + 4.0);
        }
    }

    fn collect_player_entries(&self, game: &Game, network: &NetworkSession) -> Vec<PlayerEntry> {
        let mut entries = Vec::new();
        let local_stats = &network.local_stats;
        entries.push(PlayerEntry {
            name: network.local_player_name.clone(),
            kills: local_stats.kills,
            deaths: local_stats.deaths,
            time_seconds: local_stats.time_seconds(),
            score: network.score_for_stats(local_stats),
        });

        for (peer_id, remote) in network.remote_players.iter() {
            let stats = network.remote_stats.get(peer_id).cloned().unwrap_or_default();
            entries.push(PlayerEntry {
                name: remote.name.clone(),
                kills: stats.kills,
                deaths: stats.deaths,
                time_seconds: stats.time_seconds(),
                score: network.score_for_stats(&stats),
            });
        }

        if !game.player_list_search.is_empty() {
            let query = game.player_list_search.to_ascii_lowercase();
            entries.retain(|entry| entry.name.to_ascii_lowercase().contains(&query));
        }

        match game.player_list_sort {
            1 => entries.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
            2 => entries.sort_by(|a, b| a.kills.cmp(&b.kills)),
            3 => entries.sort_by(|a, b| a.deaths.cmp(&b.deaths)),
            4 => entries.sort_by(|a, b| a.time_seconds.cmp(&b.time_seconds)),
            _ => entries.sort_by(|a, b| a.score.cmp(&b.score)),
        }
        if !game.player_list_sort_asc {
            entries.reverse();
        }

        entries
    }

    fn collect_enemy_positions(game: &Game) -> Vec<(Vec2, u8)> {
        let mut enemies = Vec::new();
        if let Some(snapshot) = &game.enemy_render_snapshot {
            for (pos, _dir, alive) in &snapshot.spider_positions {
                if *alive {
                    enemies.push((*pos, 0));
                }
            }
            for (pos, _dir, _look_dir, alive) in &snapshot.cannon_positions {
                if *alive {
                    enemies.push((*pos, 1));
                }
            }
            for (pos, _dir, _size, alive) in &snapshot.snake_positions {
                if *alive {
                    enemies.push((*pos, 2));
                }
            }
            for (pos, _dir, alive) in &snapshot.wisp_positions {
                if *alive {
                    enemies.push((*pos, 3));
                }
            }
        } else {
            for spider in &game.spiders {
                if spider.alive {
                    enemies.push((spider.pos, 0));
                }
            }
            for cannon in &game.cannons {
                if cannon.alive {
                    enemies.push((cannon.pos, 1));
                }
            }
            for snake in &game.snakes {
                if snake.alive {
                    enemies.push((snake.pos, 2));
                }
            }
            for wisp in &game.wisps {
                if wisp.alive {
                    enemies.push((wisp.pos, 3));
                }
            }
            for guardian in &game.guardians {
                if guardian.alive {
                    enemies.push((guardian.pos, 4));
                }
            }
        }
        enemies
    }

    fn render_minimap(&self, game: &Game, network: &NetworkSession) {
        if game.explored_chunks.is_empty() {
            return;
        }

        let map_size = 120.0;
        let map_padding = 10.0;
        let map_left = (self.width as f64) - map_size - map_padding;
        let portrait = game.viewport_height > game.viewport_width;
        let map_top = if game.mobile_mode || portrait { 130.0 } else { (self.height as f64) - map_size - map_padding };

        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for &(cx, cy) in &game.explored_chunks {
            min_x = min_x.min(cx);
            max_x = max_x.max(cx);
            min_y = min_y.min(cy);
            max_y = max_y.max(cy);
        }

        let world_min_x = min_x as f64 * CHUNK_SIZE as f64;
        let world_max_x = (max_x as f64 + 1.0) * CHUNK_SIZE as f64;
        let world_min_y = min_y as f64 * CHUNK_SIZE as f64;
        let world_max_y = (max_y as f64 + 1.0) * CHUNK_SIZE as f64;
        let world_span_x = (world_max_x - world_min_x).max(1.0);
        let world_span_y = (world_max_y - world_min_y).max(1.0);

        self.display_ctx.set_fill_style_str("#0f1518");
        self.display_ctx.fill_rect(map_left, map_top, map_size, map_size);
        self.display_ctx.set_stroke_style_str("#2a3a3a");
        self.display_ctx.set_line_width(1.0);
        self.display_ctx.stroke_rect(map_left, map_top, map_size, map_size);

        self.display_ctx.set_fill_style_str("#3a4a4a");
        for &(cx, cy) in &game.explored_chunks {
            let chunk_min_x = cx as f64 * CHUNK_SIZE as f64;
            let chunk_min_y = cy as f64 * CHUNK_SIZE as f64;
            let fx = (chunk_min_x - world_min_x) / world_span_x;
            let fy = (chunk_min_y - world_min_y) / world_span_y;
            let x = map_left + fx * (map_size - 2.0) + 1.0;
            let y = map_top + fy * (map_size - 2.0) + 1.0;
            let w = (CHUNK_SIZE as f64 / world_span_x) * (map_size - 2.0);
            let h = (CHUNK_SIZE as f64 / world_span_y) * (map_size - 2.0);
            self.display_ctx.fill_rect(x, y, w.max(1.0), h.max(1.0));
        }

        let enemies = Self::collect_enemy_positions(game);
        for (pos, kind) in enemies {
            let fx = (pos.x as f64 - world_min_x) / world_span_x;
            let fy = (pos.y as f64 - world_min_y) / world_span_y;
            let x = map_left + fx * (map_size - 2.0) + 1.0;
            let y = map_top + fy * (map_size - 2.0) + 1.0;
            let color = match kind {
                1 => "#dd8844",
                2 => "#55aa55",
                3 => "#c9a6ff",
                4 => "#7fa3b3",
                _ => COLOR_ACCENT2,
            };
            self.display_ctx.set_fill_style_str(color);
            self.display_ctx.fill_rect(x, y, 2.0, 2.0);
        }

        let shrine_pos = crate::game::SHRINE_POS;
        let fx = (shrine_pos.x as f64 - world_min_x) / world_span_x;
        let fy = (shrine_pos.y as f64 - world_min_y) / world_span_y;
        if fx >= 0.0 && fx <= 1.0 && fy >= 0.0 && fy <= 1.0 {
            let sx = map_left + fx * (map_size - 2.0) + 1.0;
            let sy = map_top + fy * (map_size - 2.0) + 1.0;
            self.display_ctx.set_fill_style_str("#8cd4ff");
            self.display_ctx.fill_rect(sx - 2.0, sy - 2.0, 4.0, 4.0);
        }

        let mut players = Vec::new();
        players.push((game.player.pos, COLOR_ACCENT2));
        for remote in network.remote_players.values() {
            if remote.alive {
                players.push((remote.pos, COLOR_REMOTE_PLAYER));
            }
        }
        for (pos, color) in players {
            let fx = (pos.x as f64 - world_min_x) / world_span_x;
            let fy = (pos.y as f64 - world_min_y) / world_span_y;
            let px = map_left + fx * (map_size - 2.0) + 1.0;
            let py = map_top + fy * (map_size - 2.0) + 1.0;
            self.display_ctx.set_fill_style_str(color);
            self.display_ctx.fill_rect(px - 1.0, py - 1.0, 4.0, 4.0);
        }
    }

    fn render_map_overlay(&self, game: &Game, network: &NetworkSession) {
        let (map_left, map_top, map_size) = game.map_overlay_rect();
        let map_center = game.map_center;

        self.display_ctx.set_fill_style_str("rgba(0,0,0,0.6)");
        self.display_ctx.fill_rect(0.0, 0.0, self.width as f64, self.height as f64);

        self.display_ctx.set_fill_style_str("#0d1214");
        self.display_ctx.fill_rect(map_left, map_top, map_size, map_size);
        self.display_ctx.set_stroke_style_str("#3a4a4a");
        self.display_ctx.set_line_width(2.0);
        self.display_ctx.stroke_rect(map_left, map_top, map_size, map_size);
        self.draw_overlay_close_button(map_left + map_size - 26.0, map_top + 8.0);

        let base_world_span = CHUNK_SIZE as f64 * 8.0;
        let pixels_per_world = map_size / base_world_span * game.map_zoom as f64;
        let world_half_span = map_size / (2.0 * pixels_per_world);
        let world_min_x = map_center.x as f64 - world_half_span;
        let world_max_x = map_center.x as f64 + world_half_span;
        let world_min_y = map_center.y as f64 - world_half_span;
        let world_max_y = map_center.y as f64 + world_half_span;

        let chunk_px = CHUNK_SIZE as f64 * pixels_per_world;
        if chunk_px >= 4.0 {
            let start_cx = (world_min_x / CHUNK_SIZE as f64).floor() as i32;
            let end_cx = (world_max_x / CHUNK_SIZE as f64).ceil() as i32;
            let start_cy = (world_min_y / CHUNK_SIZE as f64).floor() as i32;
            let end_cy = (world_max_y / CHUNK_SIZE as f64).ceil() as i32;
            for cy in start_cy..=end_cy {
                for cx in start_cx..=end_cx {
                    let world_x = cx as f64 * CHUNK_SIZE as f64;
                    let world_y = cy as f64 * CHUNK_SIZE as f64;
                    let x = map_left + (world_x - world_min_x) * pixels_per_world;
                    let y = map_top + (world_y - world_min_y) * pixels_per_world;
                    if game.explored_chunks.contains(&(cx, cy)) {
                        self.display_ctx.set_fill_style_str("#243036");
                    } else {
                        self.display_ctx.set_fill_style_str("#101719");
                    }
                    self.display_ctx.fill_rect(x, y, chunk_px.max(1.0), chunk_px.max(1.0));
                }
            }
        }

        let enemies = Self::collect_enemy_positions(game);
        let zoomed_out = game.map_zoom < 0.9;

        if zoomed_out {
            use std::collections::HashMap;
            let mut bins: HashMap<(i32, i32), (u32, u32)> = HashMap::new();
            let bin_size = CHUNK_SIZE as f64 * if game.map_zoom < 0.5 { 4.0 } else { 2.0 };

            for (pos, _kind) in &enemies {
                if pos.x as f64 >= world_min_x && pos.x as f64 <= world_max_x
                    && pos.y as f64 >= world_min_y && pos.y as f64 <= world_max_y
                {
                    let bx = ((pos.x as f64 - world_min_x) / bin_size).floor() as i32;
                    let by = ((pos.y as f64 - world_min_y) / bin_size).floor() as i32;
                    let entry = bins.entry((bx, by)).or_insert((0, 0));
                    entry.0 += 1;
                }
            }

            let mut players = Vec::new();
            players.push((game.player.pos, network.local_player_name.clone()));
            for remote in network.remote_players.values() {
                if remote.alive {
                    players.push((remote.pos, remote.name.clone()));
                }
            }
            for (pos, _) in &players {
                if pos.x as f64 >= world_min_x && pos.x as f64 <= world_max_x
                    && pos.y as f64 >= world_min_y && pos.y as f64 <= world_max_y
                {
                    let bx = ((pos.x as f64 - world_min_x) / bin_size).floor() as i32;
                    let by = ((pos.y as f64 - world_min_y) / bin_size).floor() as i32;
                    let entry = bins.entry((bx, by)).or_insert((0, 0));
                    entry.1 += 1;
                }
            }

            self.display_ctx.set_font("12px monospace");
            self.display_ctx.set_text_align("center");
            for ((bx, by), (enemy_count, player_count)) in bins {
                if enemy_count == 0 && player_count == 0 {
                    continue;
                }
                let center_x = map_left + (bx as f64 + 0.5) * bin_size * pixels_per_world;
                let center_y = map_top + (by as f64 + 0.5) * bin_size * pixels_per_world;
                let text = format!("E{} P{}", enemy_count, player_count);
                self.display_ctx.set_fill_style_str("#c9d4d4");
                let _ = self.display_ctx.fill_text(&text, center_x, center_y);
            }
        } else {
            for (pos, kind) in enemies {
                if pos.x as f64 >= world_min_x && pos.x as f64 <= world_max_x
                    && pos.y as f64 >= world_min_y && pos.y as f64 <= world_max_y
                {
                    let x = map_left + (pos.x as f64 - world_min_x) * pixels_per_world;
                    let y = map_top + (pos.y as f64 - world_min_y) * pixels_per_world;
                    let color = match kind {
                        1 => "#dd8844",
                        2 => "#55aa55",
                        3 => "#c9a6ff",
                        4 => "#7fa3b3",
                        _ => COLOR_ACCENT2,
                    };
                    self.display_ctx.set_fill_style_str(color);
                    self.display_ctx.fill_rect(x - 2.0, y - 2.0, 4.0, 4.0);
                }
            }

            let mut players = Vec::new();
            players.push((game.player.pos, network.local_player_name.clone(), COLOR_ACCENT2));
            for remote in network.remote_players.values() {
                if remote.alive {
                    players.push((remote.pos, remote.name.clone(), COLOR_REMOTE_PLAYER));
                }
            }

            self.display_ctx.set_font("12px monospace");
            self.display_ctx.set_text_align("left");
            for (pos, name, color) in players {
                if pos.x as f64 >= world_min_x && pos.x as f64 <= world_max_x
                    && pos.y as f64 >= world_min_y && pos.y as f64 <= world_max_y
                {
                    let x = map_left + (pos.x as f64 - world_min_x) * pixels_per_world;
                    let y = map_top + (pos.y as f64 - world_min_y) * pixels_per_world;
                    self.display_ctx.set_fill_style_str(color);
                    self.display_ctx.fill_rect(x - 2.5, y - 2.5, 5.0, 5.0);
                    if game.map_zoom >= 2.5 {
                        self.display_ctx.set_fill_style_str("#e6efef");
                        let _ = self.display_ctx.fill_text(&name, x + 6.0, y - 6.0);
                    }
                }
            }
        }

        let shrine_pos = crate::game::SHRINE_POS;
        if shrine_pos.x as f64 >= world_min_x && shrine_pos.x as f64 <= world_max_x
            && shrine_pos.y as f64 >= world_min_y && shrine_pos.y as f64 <= world_max_y
        {
            let x = map_left + (shrine_pos.x as f64 - world_min_x) * pixels_per_world;
            let y = map_top + (shrine_pos.y as f64 - world_min_y) * pixels_per_world;
            self.display_ctx.set_fill_style_str("#8cd4ff");
            self.display_ctx.begin_path();
            let _ = self.display_ctx.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
            self.display_ctx.fill();
        }

        if let Some(target) = game.map_target {
            let x = map_left + (target.x as f64 - world_min_x) * pixels_per_world;
            let y = map_top + (target.y as f64 - world_min_y) * pixels_per_world;
            let valid = !game.chunks.collides_with_obstacle(target, 4.5 * CREATURE_SCALE as f32);
            let color = if valid { "#ffd166" } else { "#cc4444" };
            self.display_ctx.set_stroke_style_str(color);
            self.display_ctx.set_line_width(2.0);
            self.display_ctx.begin_path();
            self.display_ctx.move_to(x - 6.0, y);
            self.display_ctx.line_to(x + 6.0, y);
            self.display_ctx.move_to(x, y - 6.0);
            self.display_ctx.line_to(x, y + 6.0);
            self.display_ctx.stroke();

        }

        let input_y = map_top + map_size - 6.0;
        let input_x = map_left + 10.0;
        self.display_ctx.set_fill_style_str(COLOR_LIGHT);
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("left");
        let _ = self.display_ctx.fill_text("X:", input_x, input_y);
        let is_x_active = game.map_text_input_active && game.map_active_field == 0;
        self.render_text_input_on(
            &self.display_ctx,
            input_x + 18.0,
            input_y - 12.0,
            140.0,
            &game.map_input_x,
            is_x_active,
            game.frame_count,
        );

        let _ = self.display_ctx.fill_text("Y:", input_x + 170.0, input_y);
        let is_y_active = game.map_text_input_active && game.map_active_field == 1;
        self.render_text_input_on(
            &self.display_ctx,
            input_x + 188.0,
            input_y - 12.0,
            140.0,
            &game.map_input_y,
            is_y_active,
            game.frame_count,
        );

        if game.mobile_mode {
            let button_w = 100.0;
            let button_h = 20.0;
            let button_x = map_left + map_size - button_w - 10.0;
            let button_y = input_y - 28.0;
            self.display_ctx.set_fill_style_str("#1b2a2e");
            self.display_ctx.fill_rect(button_x, button_y, button_w, button_h);
            self.display_ctx.set_stroke_style_str("#4a5a5a");
            self.display_ctx.set_line_width(1.0);
            self.display_ctx.stroke_rect(button_x, button_y, button_w, button_h);
            self.display_ctx.set_fill_style_str("#e6efef");
            self.display_ctx.set_font("12px monospace");
            self.display_ctx.set_text_align("center");
            let _ = self.display_ctx.fill_text("Teleport", button_x + button_w / 2.0, button_y + 14.0);
        }

        self.display_ctx.set_fill_style_str("#e6efef");
        self.display_ctx.set_font("12px monospace");
        self.display_ctx.set_text_align("left");
        let help_text = if game.mobile_mode {
            "Map: drag pan | +/- zoom | tap target | Teleport | X close"
        } else {
            "Map: arrows pan | Z zoom in | X zoom out | Tab switch fields | Enter teleport | M close"
        };
        let _ = self.display_ctx.fill_text(help_text, map_left, map_top - 12.0);
    }

    fn format_time(seconds: u32) -> String {
        let minutes = seconds / 60;
        let seconds = seconds % 60;
        format!("{}:{:02}", minutes, seconds)
    }

    fn render_text_input_on(
        &self,
        ctx: &CanvasRenderingContext2d,
        x: f64,
        y: f64,
        width: f64,
        text: &str,
        active: bool,
        frame_count: u32,
    ) {
        let bg_color = if active { "#222233" } else { "#111122" };
        ctx.set_fill_style_str(bg_color);
        ctx.fill_rect(x, y, width, 22.0);

        let border_color = if active { COLOR_ACCENT1 } else { "#334466" };
        ctx.set_stroke_style_str(border_color);
        ctx.set_line_width(1.0);
        ctx.stroke_rect(x, y, width, 22.0);

        ctx.set_fill_style_str(COLOR_LIGHT);
        ctx.set_font("14px monospace");
        ctx.set_text_align("left");

        let display_text = if text.is_empty() && !active { "..." } else { text };
        let _ = ctx.fill_text(display_text, x + 5.0, y + 16.0);

        if active && (frame_count / 30) % 2 == 0 {
            let cursor_x = x + 5.0 + (text.len() as f64 * 8.4);
            ctx.set_fill_style_str(COLOR_LIGHT);
            ctx.fill_rect(cursor_x, y + 4.0, 2.0, 14.0);
        }
    }

    fn render_menu_item_on(
        &self,
        ctx: &CanvasRenderingContext2d,
        x: f64,
        y: f64,
        text: &str,
        selected: bool,
        frame_count: u32,
    ) {
        ctx.set_font("18px monospace");
        ctx.set_text_align("center");

        if selected {
            let pulse = ((frame_count as f64 * 0.1).sin() * 0.3 + 0.7) as f64;
            ctx.set_global_alpha(pulse);
            ctx.set_fill_style_str(COLOR_ACCENT2);

            let _ = ctx.fill_text(">", x - 80.0, y);
            let _ = ctx.fill_text("<", x + 80.0, y);

            ctx.set_global_alpha(1.0);
            ctx.set_fill_style_str(COLOR_ACCENT2);
        } else {
            ctx.set_fill_style_str(COLOR_LIGHT);
        }

        let _ = ctx.fill_text(text, x, y);
    }

    fn render_network_status_on(&self, ctx: &CanvasRenderingContext2d, network: &NetworkSession) {
        ctx.set_font("12px monospace");
        ctx.set_text_align("right");

        let status_text = match &network.state {
            NetworkState::Disconnected => "Offline".to_string(),
            NetworkState::Connecting => "Connecting...".to_string(),
            NetworkState::WaitingForPeers => format!("Room: {} (waiting)", network.room_code),
            NetworkState::Connected => format!("Room: {} ({} players)", network.room_code, network.peer_count() + 1),
            NetworkState::Error(e) => format!("Error: {}", e),
        };

        let color = match network.state {
            NetworkState::Disconnected => "#666666",
            NetworkState::Connecting => "#ffff00",
            NetworkState::WaitingForPeers => "#00ff00",
            NetworkState::Connected => "#00ff00",
            NetworkState::Error(_) => "#ff0000",
        };

        ctx.set_fill_style_str(color);
        let _ = ctx.fill_text(&status_text, (self.width - 10) as f64, 25.0);
    }

    fn render_gameover(&self, game: &Game) {
        // Game over text
        self.ctx.set_fill_style_str(COLOR_ACCENT2);
        self.ctx.set_font("bold 48px monospace");
        self.ctx.set_text_align("center");
        let _ = self.ctx.fill_text("GAME OVER", (self.width / 2) as f64, (self.height / 2 - 50) as f64);

        // Stats
        self.ctx.set_fill_style_str(COLOR_LIGHT);
        self.ctx.set_font("24px monospace");
        let _ = self.ctx.fill_text(
            &format!("Wave: {}", game.wave),
            (self.width / 2) as f64,
            (self.height / 2 + 20) as f64,
        );
        let _ = self.ctx.fill_text(
            &format!("Kills: {}", game.kills),
            (self.width / 2) as f64,
            (self.height / 2 + 55) as f64,
        );

        let time = (game.end_frame - game.start_frame) / 60;
        let _ = self.ctx.fill_text(
            &format!("Time: {}s", time),
            (self.width / 2) as f64,
            (self.height / 2 + 90) as f64,
        );

        // Instructions
        self.ctx.set_font("16px monospace");
        let help_text = if game.mobile_mode {
            "Tap screen to open map"
        } else {
            "Press Z or SPACE to continue"
        };
        let _ = self.ctx.fill_text(
            help_text,
            (self.width / 2) as f64,
            (self.height - 80) as f64,
        );
    }

    fn fill_circle(&self, x: f64, y: f64, radius: f64) {
        self.ctx.begin_path();
        let _ = self.ctx.arc(x, y, radius, 0.0, std::f64::consts::PI * 2.0);
        self.ctx.fill();
    }

    fn fill_oval(&self, x: f64, y: f64, width: f64, height: f64) {
        self.ctx.begin_path();
        let _ = self.ctx.ellipse(
            x, y,
            width / 2.0, height / 2.0,
            0.0, 0.0, std::f64::consts::PI * 2.0,
        );
        self.ctx.fill();
    }

    fn draw_pixel(&self, x: f64, y: f64) {
        self.ctx.fill_rect(x, y, 1.0, 1.0);
    }

    fn draw_rect_filled(&self, x: f64, y: f64, width: f64, height: f64) {
        self.ctx.fill_rect(x, y, width.max(1.0), height.max(1.0));
    }

    fn draw_circle_filled(&self, cx: f64, cy: f64, radius: f64) {
        let r = radius.max(1.0) as i32;
        let cx = cx as i32;
        let cy = cy as i32;
        for dy in -r..=r {
            let dx = ((r * r - dy * dy) as f64).sqrt().floor() as i32;
            let y = cy + dy;
            let x0 = cx - dx;
            let w = (dx * 2 + 1) as f64;
            self.ctx.fill_rect(x0 as f64, y as f64, w, 1.0);
        }
    }

    fn draw_circle_outline(&self, cx: f64, cy: f64, radius: f64) {
        let mut x = radius.max(1.0) as i32;
        let mut y = 0i32;
        let mut err = 1 - x;
        let cx = cx.round() as i32;
        let cy = cy.round() as i32;

        while x >= y {
            self.draw_pixel((cx + x) as f64, (cy + y) as f64);
            self.draw_pixel((cx + y) as f64, (cy + x) as f64);
            self.draw_pixel((cx - y) as f64, (cy + x) as f64);
            self.draw_pixel((cx - x) as f64, (cy + y) as f64);
            self.draw_pixel((cx - x) as f64, (cy - y) as f64);
            self.draw_pixel((cx - y) as f64, (cy - x) as f64);
            self.draw_pixel((cx + y) as f64, (cy - x) as f64);
            self.draw_pixel((cx + x) as f64, (cy - y) as f64);

            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x + 1);
            }
        }
    }

    fn draw_circle_outline_masked(
        &self,
        cx: f64,
        cy: f64,
        radius: f64,
        mask_cx: f64,
        mask_cy: f64,
        mask_radius: f64,
    ) {
        let mut x = radius.max(1.0) as i32;
        let mut y = 0i32;
        let mut err = 1 - x;
        let cx = cx.round() as i32;
        let cy = cy.round() as i32;
        let mask_cx = mask_cx.round() as i32;
        let mask_cy = mask_cy.round() as i32;
        let mask_r2 = (mask_radius.max(1.0) as i32).pow(2);

        while x >= y {
            self.draw_pixel_if_outside_mask(cx + x, cy + y, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx + y, cy + x, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx - y, cy + x, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx - x, cy + y, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx - x, cy - y, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx - y, cy - x, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx + y, cy - x, mask_cx, mask_cy, mask_r2);
            self.draw_pixel_if_outside_mask(cx + x, cy - y, mask_cx, mask_cy, mask_r2);

            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x + 1);
            }
        }
    }

    fn draw_pixel_if_outside_mask(
        &self,
        x: i32,
        y: i32,
        mask_cx: i32,
        mask_cy: i32,
        mask_r2: i32,
    ) {
        let dx = x - mask_cx;
        let dy = y - mask_cy;
        if dx * dx + dy * dy > mask_r2 {
            self.draw_pixel(x as f64, y as f64);
        }
    }

    fn draw_line(&self, x0: f64, y0: f64, x1: f64, y1: f64) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            self.ctx.fill_rect(x0 as f64, y0 as f64, 1.0, 1.0);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn draw_line_scaled(&self, x0: f64, y0: f64, x1: f64, y1: f64, thickness: f64) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let size = thickness.round().max(1.0) as i32;
        let half = (size - 1) / 2;

        loop {
            self.ctx.fill_rect(
                (x0 - half) as f64,
                (y0 - half) as f64,
                size as f64,
                size as f64,
            );
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

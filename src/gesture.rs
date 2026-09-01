/// Minimum displacement (px) to distinguish a click from a gesture.
const CLICK_THRESHOLD_PX: f64 = 10.0;

/// Minimum accumulated displacement (px) before a new direction segment is emitted.
const SEGMENT_THRESHOLD_PX: f64 = 30.0;

/// The eight directions used for gesture quantization, in clockwise order.
const DIRECTIONS: [&str; 8] = ["R", "DR", "D", "DL", "L", "UL", "U", "UR"];

/// Tracker for mouse gesture recognition.
///
/// Collects points while the gesture button is held, then quantizes the
/// movement path into a compact direction string (e.g. "R,DR,U") on finish.
#[derive(Debug, Default)]
pub struct GestureTracker {
    start: Option<(f64, f64)>,
    last: Option<(f64, f64)>,
    acc_x: f64,
    acc_y: f64,
    dirs: Vec<usize>,
}

/// Outcome of a finished gesture.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Movement below the click threshold; should behave like a normal click.
    Click,
    /// A recognized gesture, encoded as a direction string.
    Gesture(String),
}

impl GestureTracker {
    /// Start tracking from the given position.
    pub fn start(&mut self, x: f64, y: f64) {
        self.start = Some((x, y));
        self.last = Some((x, y));
        self.acc_x = 0.0;
        self.acc_y = 0.0;
        self.dirs.clear();
    }

    /// Feed a new pointer position while the button is held.
    pub fn add(&mut self, x: f64, y: f64) {
        let Some((lx, ly)) = self.last else {
            return;
        };
        self.acc_x += x - lx;
        self.acc_y += y - ly;
        self.last = Some((x, y));

        let dist = (self.acc_x * self.acc_x + self.acc_y * self.acc_y).sqrt();
        if dist >= SEGMENT_THRESHOLD_PX {
            let dir = direction_index(self.acc_x, self.acc_y);
            if self.dirs.last() != Some(&dir) {
                self.dirs.push(dir);
            }
            self.acc_x = 0.0;
            self.acc_y = 0.0;
        }
    }

    /// Finish tracking at the given final position and decide the outcome.
    pub fn finish(&mut self, x: f64, y: f64) -> Option<Outcome> {
        let start = self.start?;
        self.add(x, y);
        self.start = None;

        let dx = x - start.0;
        let dy = y - start.1;
        let total = (dx * dx + dy * dy).sqrt();
        if total < CLICK_THRESHOLD_PX {
            return Some(Outcome::Click);
        }

        let dirs = self.dirs.clone();
        if dirs.is_empty() {
            return Some(Outcome::Gesture(direction_str(dx, dy).to_string()));
        }
        let gesture = dirs
            .iter()
            .map(|&d| DIRECTIONS[d])
            .collect::<Vec<_>>()
            .join(",");
        Some(Outcome::Gesture(gesture))
    }
}

/// Map a displacement vector to the nearest of the 8 direction buckets.
fn direction_index(dx: f64, dy: f64) -> usize {
    // atan2 returns (-PI, PI]; shift so that "R" (0 rad) is the zero bucket.
    let angle = dy.atan2(dx).to_degrees();
    let normalized = (angle + 360.0) % 360.0;
    // Bucket size is 45 degrees; round to nearest multiple of 45.
    let idx = ((normalized / 45.0).round() as i32).rem_euclid(8);
    idx as usize
}

/// Direction string for a single displacement vector.
fn direction_str(dx: f64, dy: f64) -> &'static str {
    DIRECTIONS[direction_index(dx, dy)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_movement_is_click() {
        let mut t = GestureTracker::default();
        t.start(100.0, 100.0);
        t.add(104.0, 102.0);
        assert_eq!(t.finish(106.0, 103.0), Some(Outcome::Click));
    }

    #[test]
    fn straight_right_is_r() {
        let mut t = GestureTracker::default();
        t.start(0.0, 0.0);
        t.add(50.0, 1.0);
        assert_eq!(t.finish(100.0, 0.0), Some(Outcome::Gesture("R".into())));
    }

    #[test]
    fn right_then_up_is_r_u() {
        let mut t = GestureTracker::default();
        t.start(0.0, 0.0);
        t.add(60.0, 0.0);
        // Sharp corner: move almost straight up after reaching the right.
        t.add(62.0, -60.0);
        t.add(62.0, -120.0);
        assert_eq!(
            t.finish(62.0, -140.0),
            Some(Outcome::Gesture("R,U".into()))
        );
    }

    #[test]
    fn consecutive_same_directions_merge() {
        let mut t = GestureTracker::default();
        t.start(0.0, 0.0);
        t.add(40.0, 0.0);
        t.add(70.0, 0.0);
        assert_eq!(t.finish(100.0, 0.0), Some(Outcome::Gesture("R".into())));
    }
}
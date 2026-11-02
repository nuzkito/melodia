//! The wait times of the backends: the real backend before each follow-up, the demo before each
//! simulated answer. They are in one struct, so that the tests can make them shorter
//! ([`Timings::scaled`]). The app always uses [`Timings::default`].

use std::time::Duration;

use crate::playback::EXPECTATION_TTL;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// After a command that the player page will not confirm, the backend asks Spotify what plays
    /// after this time.
    pub state_after_command: Duration,
    /// After a request to Spotify for what plays fails, the time until the backend asks again.
    pub fetch_retry_after: Duration,
    /// The page said that the music left this app, but Spotify still shows it here. The backend
    /// asks again after this time.
    pub where_retry_after: Duration,
    /// The time after a report that the page cannot fully explain, until the backend asks
    /// Spotify. It is a moment, so that Spotify knows what the page saw.
    pub state_after_page_report: Duration,
    /// The interval at which the backend saves the position of the music that plays here, and
    /// makes sure that the web player starts. The backend asks Spotify nothing.
    pub tick: Duration,
    /// After the music moves to this app, the maximum time for Spotify to show it as the device.
    /// During this time, the backend checks the reports of the page with Spotify.
    pub moving_here_window: Duration,
    /// After the music moves to this app, the backend asks Spotify after this time if it plays
    /// here. The page usually tells this before (`PageState::here`).
    pub here_check_after: Duration,
    /// The device picker asks for the list every 10 s while it is open. For this time after its
    /// request, it counts as open.
    pub devices_shown_for: Duration,
    /// After a transfer, the time during which answers and device lists from Spotify that still
    /// show a different active device count as old.
    pub transfer_hold: Duration,
    /// The wait between the requests to a new device while Spotify does not know it yet.
    pub new_device_retry_every: Duration,
    /// The time during which a new volume shows over answers from Spotify that still report the
    /// old volume.
    pub volume_hold: Duration,
    /// The interval between saves of the position while the music plays in this app.
    pub resume_save_every: Duration,
    /// The time after this app becomes the device during which the backend still restores the
    /// saved position.
    pub resume_window: Duration,
    /// The time during which answers from Spotify that do not show the result of a command yet
    /// count as old (see [`crate::playback::Expectation`]).
    pub expectation_ttl: Duration,
    /// The time for the demo to log in.
    pub demo_login: Duration,
    /// The time for the demo to answer a search.
    pub demo_search: Duration,
    /// The time for the simulated web player of the demo to start.
    pub demo_web_player: Duration,
    /// The time for the demo to create a playlist.
    pub demo_create_playlist: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        let tick = Duration::from_secs(5);
        Self {
            state_after_command: Duration::from_millis(400),
            fetch_retry_after: Duration::from_secs(5),
            where_retry_after: Duration::from_secs(1),
            state_after_page_report: Duration::from_millis(300),
            tick,
            moving_here_window: Duration::from_secs(5),
            here_check_after: Duration::from_secs(2),
            devices_shown_for: Duration::from_secs(15),
            transfer_hold: Duration::from_secs(3),
            new_device_retry_every: Duration::from_millis(250),
            volume_hold: Duration::from_millis(2500),
            resume_save_every: tick,
            resume_window: Duration::from_secs(10),
            expectation_ttl: EXPECTATION_TTL,
            demo_login: Duration::from_millis(700),
            demo_search: Duration::from_millis(350),
            demo_web_player: Duration::from_millis(900),
            demo_create_playlist: Duration::from_millis(600),
        }
    }
}

impl Timings {
    /// All timings multiplied by `factor` (for example `0.2` in the tests).
    pub fn scaled(factor: f64) -> Self {
        let d = Self::default();
        let scale = |duration: Duration| duration.mul_f64(factor);
        Self {
            state_after_command: scale(d.state_after_command),
            fetch_retry_after: scale(d.fetch_retry_after),
            where_retry_after: scale(d.where_retry_after),
            state_after_page_report: scale(d.state_after_page_report),
            tick: scale(d.tick),
            moving_here_window: scale(d.moving_here_window),
            here_check_after: scale(d.here_check_after),
            devices_shown_for: scale(d.devices_shown_for),
            transfer_hold: scale(d.transfer_hold),
            new_device_retry_every: scale(d.new_device_retry_every),
            volume_hold: scale(d.volume_hold),
            resume_save_every: scale(d.resume_save_every),
            resume_window: scale(d.resume_window),
            expectation_ttl: scale(d.expectation_ttl),
            demo_login: scale(d.demo_login),
            demo_search: scale(d.demo_search),
            demo_web_player: scale(d.demo_web_player),
            demo_create_playlist: scale(d.demo_create_playlist),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_shortens_every_timing_alike() {
        let short = Timings::scaled(0.1);
        assert_eq!(short.tick, Duration::from_millis(500));
        assert_eq!(short.expectation_ttl, EXPECTATION_TTL / 10);
        assert_eq!(Timings::scaled(1.0), Timings::default());
    }
}

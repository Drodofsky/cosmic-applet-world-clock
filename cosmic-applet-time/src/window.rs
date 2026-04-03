// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::{
    Element, Task, app,
    applet::cosmic_panel_config::PanelAnchor,
    cctk::sctk::reexports::calloop,
    iced::{
        Alignment, Length, Rectangle, Subscription,
        futures::{SinkExt, StreamExt, channel::mpsc},
        widget::{column, row},
        window,
    },
    iced_futures::stream,
    iced_widget::Column,
    widget::{Id, autosize, button, container, rectangle_tracker::*, space},
};
use jiff::{Timestamp, Zoned, civil::Date, tz::TimeZone};
use logind_zbus::manager::ManagerProxy;
use std::hash::Hash;
use std::sync::LazyLock;
use timedate_zbus::TimeDateProxy;
use tokio::{sync::watch, time};

use cosmic::applet::token::subscription::{
    TokenRequest, TokenUpdate, activation_token_subscription,
};
use icu::{
    datetime::{
        DateTimeFormatter, DateTimeFormatterPreferences, fieldsets,
        input::{Date as IcuDate, DateTime, Time},
    },
    locale::Locale,
};

static AUTOSIZE_MAIN_ID: LazyLock<Id> = LazyLock::new(|| Id::new("autosize-main"));

// Specifiers for strftime that indicate seconds. Subsecond precision isn't supported by the applet
// so those specifiers aren't listed here. This list is non-exhaustive, and it's possible that %X
// and other specifiers have to be added depending on locales.
fn get_system_locale() -> Locale {
    for var in ["LC_TIME", "LC_ALL", "LANG"] {
        if let Ok(locale_str) = std::env::var(var) {
            let cleaned_locale = locale_str
                .split('.')
                .next()
                .unwrap_or(&locale_str)
                .replace('_', "-");

            if let Ok(locale) = Locale::try_from_str(&cleaned_locale) {
                return locale;
            }

            // Try language-only fallback (e.g., "en" from "en-US")
            if let Some(lang) = cleaned_locale.split('-').next() {
                if let Ok(locale) = Locale::try_from_str(lang) {
                    return locale;
                }
            }
        }
    }
    tracing::warn!("No valid locale found in environment, using fallback");
    Locale::try_from_str("en-US").expect("Failed to parse fallback locale 'en-US'")
}

pub struct Window {
    core: cosmic::app::Core,
    popup: Option<window::Id>,
    now: Zoned,
    timezone: Option<TimeZone>,
    date_today: Date,
    date_selected: Date,
    rectangle_tracker: Option<RectangleTracker<u32>>,
    rectangle: Rectangle,
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
    show_seconds_tx: watch::Sender<bool>,
    locale: Locale,
}

#[derive(Debug, Clone)]
pub enum Message {
    CloseRequested(window::Id),
    Tick,
    Rectangle(RectangleUpdate<u32>),
    Token(TokenUpdate),
    TimezoneUpdate(String),
}

impl Window {
    fn create_datetime(&self, date: &Date) -> DateTime<icu::calendar::Gregorian> {
        DateTime {
            date: IcuDate::try_new_gregorian(
                date.year() as i32,
                date.month() as u8,
                date.day() as u8,
            )
            .unwrap(),
            time: Time::try_new(
                self.now.hour() as u8,
                self.now.minute() as u8,
                self.now.second() as u8,
                0,
            )
            .unwrap(),
        }
    }

    fn vertical_layout(&self) -> Element<'_, Message> {
        let mut elements = Vec::new();
        let date = self.now.date();
        let datetime = self.create_datetime(&date);
        let prefs = DateTimeFormatterPreferences::from(self.locale.clone());

        let fs = fieldsets::T::medium();

        let formatted_time = DateTimeFormatter::try_new(prefs, fs)
            .unwrap()
            .format(&datetime)
            .to_string();

        // todo: split using formatToParts when it is implemented
        // https://github.com/unicode-org/icu4x/issues/4936#issuecomment-2128812667
        for p in formatted_time.split_whitespace().flat_map(|s| s.split(':')) {
            elements.push(self.core.applet.text(p.to_owned()).into());
        }

        let date_time_col = Column::with_children(elements)
            .align_x(Alignment::Center)
            .spacing(4);

        Element::from(
            column!(
                date_time_col,
                space::horizontal().width(Length::Fixed(
                    (self.core.applet.suggested_size(true).0
                        + 2 * self.core.applet.suggested_padding(true).1)
                        as f32
                ))
            )
            .align_x(Alignment::Center),
        )
    }

    fn horizontal_layout(&self) -> Element<'_, Message> {
        Element::from(
            row!(
                self.core.applet.text("Tokyo 18:43"),
                container(space::vertical().height(Length::Fixed(
                    (self.core.applet.suggested_size(true).1
                        + 2 * self.core.applet.suggested_padding(true).1)
                        as f32
                )))
            )
            .align_y(Alignment::Center),
        )
    }
}

impl cosmic::Application for Window {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &str = "com.system76.CosmicAppletTime";

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        let locale = get_system_locale();
        let now = Zoned::now();
        // get today's date for highlighting purposes
        let today = now.date();

        // Synch `show_seconds` from the config within the time subscription
        let (show_seconds_tx, _) = watch::channel(true);

        (
            Self {
                core,
                popup: None,
                now,
                timezone: None,
                date_today: today,
                date_selected: today,
                rectangle_tracker: None,
                rectangle: Rectangle::default(),
                token_tx: None,
                show_seconds_tx,
                locale,
            },
            Task::none(),
        )
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Message> {
        fn time_subscription(show_seconds: watch::Receiver<bool>) -> Subscription<Message> {
            struct Wrapper {
                inner: watch::Receiver<bool>,
                id: &'static str,
            }
            impl Hash for Wrapper {
                fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                    self.id.hash(state);
                }
            }
            Subscription::run_with(
                Wrapper {
                    inner: show_seconds,
                    id: "time-sub",
                },
                |Wrapper { inner, id: _ }| {
                    let mut show_seconds = inner.clone();
                    stream::channel(1, move |mut output: mpsc::Sender<Message>| async move {
                        // Mark this receiver's state as changed so that it always receives an initial
                        // update during the loop below
                        // This allows us to avoid duplicating code from the loop
                        show_seconds.mark_changed();
                        let mut period = 1;
                        let mut timer = time::interval(time::Duration::from_secs(period));
                        timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

                        loop {
                            tokio::select! {
                                    _ = timer.tick() => {
                                        #[cfg(debug_assertions)]
                                        if let Err(err) = output.send(Message::Tick).await {
                                            tracing::error!(?err, "Failed sending tick request to applet");
                                        }
                                        #[cfg(not(debug_assertions))]
                                        let _ = output.send(Message::Tick).await;

                                        // Calculate a delta if we're ticking per minute to keep ticks stable
                                        // Based on i3status-rust
                                        let current = Timestamp::now().as_second() as u64 % period;
                                        if current != 0 {
                                            timer.reset_after(time::Duration::from_secs(period - current));
                                        }
                                    },
                                // Update timer if the user toggles show_seconds
                                Ok(()) = show_seconds.changed() => {
                                    let seconds = *show_seconds.borrow_and_update();
                                    if seconds {
                                        period = 1;
                                        // Subsecond precision isn't needed; skip calculating offset
                                        let period = time::Duration::from_secs(period);
                                        let start = time::Instant::now() + period;
                                        timer = time::interval_at(start, period);
                                    } else {
                                        period = 60;
                                        let delta = time::Duration::from_secs(period - Timestamp::now().as_second() as u64 % period);
                                        let now = time::Instant::now();
                                        // Start ticking from the next minute to update the time properly
                                        let start = now + delta;
                                        let period = time::Duration::from_secs(period);
                                        timer = time::interval_at(start, period);

                                        timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
                                    }
                                }
                            }
                        }
                    })
                },
            )
        }

        // Update applet's timezone if the system's timezone changes
        async fn timezone_update(output: &mut mpsc::Sender<Message>) -> zbus::Result<()> {
            let conn = zbus::Connection::system().await?;
            let proxy = TimeDateProxy::new(&conn).await?;

            // The stream always returns the current timezone as its first item even if it wasn't
            // updated. If the proxy is recreated in a loop somehow, the resulting stream will
            // always yield an update immediately which could lead to spammed false updates.
            let mut stream_tz = proxy.receive_timezone_changed().await;
            while let Some(property) = stream_tz.next().await {
                let tz = property.get().await?;
                output
                    .send(Message::TimezoneUpdate(tz))
                    .await
                    .map_err(|e| {
                        zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::other(e)))
                    })?;
            }
            Ok(())
        }

        fn timezone_subscription() -> Subscription<Message> {
            Subscription::run_with("timezone-sub", |_| {
                stream::channel(1, |mut output| async move {
                    'retry: loop {
                        match timezone_update(&mut output).await {
                            Ok(()) => break 'retry,
                            Err(err) => {
                                tracing::error!(
                                    ?err,
                                    "Automatic timezone updater failed; retrying in one minute"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            }
                        }
                    }

                    std::future::pending().await
                })
            })
        }

        // Update the time when waking from sleep
        async fn wake_from_sleep(output: &mut mpsc::Sender<Message>) -> zbus::Result<()> {
            let connection = zbus::Connection::system().await?;
            let proxy = ManagerProxy::new(&connection).await?;

            while let Some(property) = proxy.receive_prepare_for_sleep().await?.next().await {
                let waking = !property.args()?.start();
                if waking {
                    let _ = output.send(Message::Tick).await;
                }
            }
            Ok(())
        }

        fn wake_from_sleep_subscription() -> Subscription<Message> {
            Subscription::run_with("wake-from-suspend-sub", |_| {
                stream::channel(1, |mut output| async move {
                    if let Err(err) = wake_from_sleep(&mut output).await {
                        tracing::error!(?err, "Failed to subscribe to wake-from-sleep signal");
                    }
                })
            })
        }

        let show_seconds_rx = self.show_seconds_tx.subscribe();
        Subscription::batch([
            rectangle_tracker_subscription(0).map(|e| Message::Rectangle(e.1)),
            time_subscription(show_seconds_rx),
            activation_token_subscription(0).map(Message::Token),
            timezone_subscription(),
            wake_from_sleep_subscription(),
        ])
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::Tick => {
                self.now = self.timezone.as_ref().map_or_else(
                    || Zoned::now(),
                    |tz| Zoned::now().with_time_zone(tz.clone()),
                );
                Task::none()
            }
            Message::Rectangle(u) => {
                match u {
                    RectangleUpdate::Rectangle(r) => {
                        self.rectangle = r.1;
                    }
                    RectangleUpdate::Init(tracker) => {
                        self.rectangle_tracker = Some(tracker);
                    }
                }
                Task::none()
            }
            Message::CloseRequested(id) => {
                if Some(id) == self.popup {
                    self.popup = None;
                }
                Task::none()
            }
            Message::Token(u) => {
                match u {
                    TokenUpdate::Init(tx) => {
                        self.token_tx = Some(tx);
                    }
                    TokenUpdate::Finished => {
                        self.token_tx = None;
                    }
                    TokenUpdate::ActivationToken { token, .. } => {
                        let mut cmd = std::process::Command::new("cosmic-settings");
                        cmd.arg("time");
                        if let Some(token) = token {
                            cmd.env("XDG_ACTIVATION_TOKEN", &token);
                            cmd.env("DESKTOP_STARTUP_ID", &token);
                        }
                        tokio::spawn(cosmic::process::spawn(cmd));
                    }
                }
                Task::none()
            }
            Message::TimezoneUpdate(timezone) => {
                if let Ok(timezone) = TimeZone::get(&timezone) {
                    self.now = Zoned::now().with_time_zone(timezone.clone());
                    self.date_today = self.now.date();
                    self.date_selected = self.date_today;
                    self.timezone = Some(timezone);
                }

                self.update(Message::Tick)
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let horizontal = matches!(
            self.core.applet.anchor,
            PanelAnchor::Top | PanelAnchor::Bottom
        );

        let button = button::custom(if horizontal {
            self.horizontal_layout()
        } else {
            self.vertical_layout()
        })
        .padding(if horizontal {
            [0, self.core.applet.suggested_padding(true).0]
        } else {
            [self.core.applet.suggested_padding(true).0, 0]
        })
        .class(cosmic::theme::Button::AppletIcon);

        autosize::autosize(
            if let Some(tracker) = self.rectangle_tracker.as_ref() {
                Element::from(tracker.container(0, button).ignore_bounds(true))
            } else {
                button.into()
            },
            AUTOSIZE_MAIN_ID.clone(),
        )
        .into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }
}

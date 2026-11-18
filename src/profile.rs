use anyhow::{Context, Result, bail};
use gst::prelude::*;
use gtk::{
    gio,
    glib::{self, subclass::prelude::*},
};
use once_cell::sync::OnceCell as OnceLock;
use serde::Deserialize;

use crate::encoder::H264Encoder;

const DEFAULT_SUGGESTED_MAX_FRAMERATE: gst::Fraction = gst::Fraction::from_integer(60);
const MAX_THREAD_COUNT: u32 = 64;

#[derive(Debug, Deserialize)]
struct Profiles {
    supported: Vec<ProfileData>,
    experimental: Vec<ProfileData>,
}

#[derive(Debug, Deserialize)]
struct ProfileData {
    id: String,
    #[serde(default)]
    is_experimental: bool,
    name: String,
    #[serde(rename = "suggested-max-fps")]
    suggested_max_framerate: Option<f64>,
    #[serde(rename = "extension")]
    file_extension: String,
    #[serde(rename = "videoenc")]
    videoenc_bin_str: String,
    #[serde(rename = "audioenc")]
    audioenc_bin_str: Option<String>,
    #[serde(rename = "muxer")]
    muxer_bin_str: Option<String>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Profile {
        pub(super) data: OnceLock<ProfileData>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Profile {
        const NAME: &'static str = "KoohaProfile";
        type Type = super::Profile;
    }

    impl ObjectImpl for Profile {}
}

glib::wrapper! {
    pub struct Profile(ObjectSubclass<imp::Profile>);
}

impl Profile {
    fn from_data(data: ProfileData) -> Self {
        let this = glib::Object::new::<Self>();
        this.imp().data.set(data).unwrap();
        this
    }

    fn data(&self) -> &ProfileData {
        self.imp().data.get().unwrap()
    }

    pub fn all() -> Result<&'static [Self]> {
        static ALL: OnceLock<Vec<Profile>> = OnceLock::new();

        ALL.get_or_try_init(|| {
            let bytes = gio::resources_lookup_data(
                "/io/github/seadve/Kooha/profiles.yml",
                gio::ResourceLookupFlags::NONE,
            )?;
            let profiles = serde_yaml::from_slice::<Profiles>(&bytes)?;

            let supported = profiles.supported.into_iter().map(|mut data| {
                data.is_experimental = false;
                Self::from_data(data)
            });
            let experimental = profiles.experimental.into_iter().map(|mut data| {
                data.is_experimental = true;
                Self::from_data(data)
            });
            Ok(supported.chain(experimental).collect())
        })
        .map(|v| v.as_slice())
    }

    pub fn from_id(id: &str) -> Result<&'static Self> {
        let profile = Self::all()?
            .iter()
            .find(|p| p.id() == id)
            .with_context(|| format!("Profile `{}` not found", id))?;
        Ok(profile)
    }

    pub fn id(&self) -> &str {
        &self.data().id
    }

    pub fn name(&self) -> &str {
        &self.data().name
    }

    pub fn file_extension(&self) -> &str {
        &self.data().file_extension
    }

    pub fn supports_audio(&self) -> bool {
        self.data().audioenc_bin_str.is_some()
    }

    pub fn suggested_max_framerate(&self) -> gst::Fraction {
        self.data().suggested_max_framerate.map_or_else(
            || DEFAULT_SUGGESTED_MAX_FRAMERATE,
            |raw| gst::Fraction::approximate_f64(raw).unwrap(),
        )
    }

    pub fn is_experimental(&self) -> bool {
        self.data().is_experimental
    }

    /// Whether the video encoder is the H.264 encoder of the recording, the
    /// `${H264_ENCODER}` marker.
    pub fn uses_h264_encoder(&self) -> bool {
        self.data().videoenc_bin_str.contains("${H264_ENCODER}")
    }

    /// The factory names of the elements of the encoder and muxer bins: the
    /// first token of each `!`-separated part, without caps or properties.
    /// The H.264 encoder counts as the CPU one.
    pub fn required_elements(&self) -> Vec<String> {
        let data = self.data();

        let videoenc_bin_str = expand(&data.videoenc_bin_str, H264Encoder::Cpu);

        [
            Some(&videoenc_bin_str),
            data.audioenc_bin_str.as_ref(),
            data.muxer_bin_str.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|description| description.split('!'))
        .filter_map(|part| part.split_whitespace().next())
        .map(str::to_string)
        .collect()
    }

    pub fn is_available(&self) -> bool {
        self.is_available_inner()
            .inspect_err(|err| {
                tracing::debug!("Profile `{}` is not available: {:?}", self.id(), err);
            })
            .is_ok()
    }

    /// Whether the profile parses, with the CPU encoder for H.264: a hardware
    /// encoder is checked when recording.
    fn is_available_inner(&self) -> Result<()> {
        parse_bin_test(&self.data().videoenc_bin_str).context("Failed to parse videoenc bin")?;

        if let Some(audioenc_bin_str) = &self.data().audioenc_bin_str {
            parse_bin_test(audioenc_bin_str).context("Failed to parse audioenc bin")?;
        }

        if let Some(muxer_bin_str) = &self.data().muxer_bin_str {
            parse_bin_test(muxer_bin_str).context("Failed to parse muxer bin")?;
        }

        Ok(())
    }

    /// `attach_with` the CPU encoder.
    #[cfg(test)]
    pub fn attach(
        &self,
        pipeline: &gst::Pipeline,
        video_src: &gst::Element,
        audio_srcs: Option<&gst::Element>,
        sink: &gst::Element,
    ) -> Result<()> {
        self.attach_with(pipeline, video_src, audio_srcs, sink, H264Encoder::Cpu)
    }

    /// Links the encoder and muxer bins between the sources and `sink`, with
    /// `encoder` for the H.264 profiles.
    pub fn attach_with(
        &self,
        pipeline: &gst::Pipeline,
        video_src: &gst::Element,
        audio_srcs: Option<&gst::Element>,
        sink: &gst::Element,
        encoder: H264Encoder,
    ) -> Result<()> {
        let videoenc_bin = parse_bin_inner(
            "kooha-videoenc-bin",
            &self.data().videoenc_bin_str,
            true,
            encoder,
        )?;
        debug_assert!(videoenc_bin.iterate_elements().into_iter().any(|element| {
            let factory = element.unwrap().factory().unwrap();
            factory.has_type(gst::ElementFactoryType::VIDEO_ENCODER)
        }));

        pipeline.add(&videoenc_bin)?;
        video_src.link(&videoenc_bin)?;

        match (&self.data().audioenc_bin_str, &self.data().muxer_bin_str) {
            (None, None) => {
                // Special case for gifenc

                if audio_srcs.is_some() {
                    tracing::error!("Audio srcs ignored: Profile does not support audio");
                }

                videoenc_bin.link(sink)?;
            }
            (audioenc_str, Some(muxer_bin_str)) => {
                let muxer_bin = parse_bin("kooha-muxer-bin", muxer_bin_str)?;
                let muxer = muxer_bin
                    .iterate_elements()
                    .find(|element| {
                        element
                            .factory()
                            .is_some_and(|f| f.has_type(gst::ElementFactoryType::MUXER))
                    })
                    .context("Can't find the muxer in muxer bin")?;

                pipeline.add(&muxer_bin)?;
                videoenc_bin.link_pads(None, &muxer, Some("video_%u"))?;
                muxer_bin.link(sink)?;

                if let Some(audio_srcs) = audio_srcs {
                    let audioenc_str = audioenc_str
                        .as_ref()
                        .context("Failed to handle audio srcs: Profile has no audio encoder")?;
                    let audioenc_bin = parse_bin("kooha-audioenc-bin", audioenc_str)?;
                    debug_assert!(audioenc_bin.iterate_elements().into_iter().any(|element| {
                        let factory = element.unwrap().factory().unwrap();
                        factory.has_type(gst::ElementFactoryType::AUDIO_ENCODER)
                    }));

                    pipeline.add(&audioenc_bin)?;
                    audio_srcs.link(&audioenc_bin)?;
                    audioenc_bin.link_pads(None, &muxer, Some("audio_%u"))?;
                }
            }
            (Some(_), None) => {
                bail!("Unexpected audioenc without muxer")
            }
        }

        Ok(())
    }
}

/// Replaces `${H264_ENCODER}` with the chain of `encoder`, then
/// `${N_THREADS}` with the ideal thread count.
pub fn expand(description: &str, encoder: H264Encoder) -> String {
    let ideal_n_threads = glib::num_processors().min(MAX_THREAD_COUNT);
    description
        .replace("${H264_ENCODER}", encoder.chain())
        .replace("${N_THREADS}", &ideal_n_threads.to_string())
}

fn parse_bin_test(description: &str) -> Result<(), glib::Error> {
    // Empty names are ignored in implementation details of `gst::parse::bin_from_description_with_name_full`
    parse_bin_inner("", description, false, H264Encoder::Cpu)?;

    Ok(())
}

fn parse_bin(name: &str, description: &str) -> Result<gst::Bin, glib::Error> {
    parse_bin_inner(name, description, true, H264Encoder::Cpu)
}

fn parse_bin_inner(
    name: &str,
    description: &str,
    add_ghost_pads: bool,
    encoder: H264Encoder,
) -> Result<gst::Bin, glib::Error> {
    let formatted_description = expand(description, encoder);
    let bin = gst::parse::bin_from_description_with_name_full(
        &formatted_description,
        add_ghost_pads,
        name,
        None,
        gst::ParseFlags::FATAL_ERRORS,
    )?
    .downcast()
    .unwrap();
    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::settings::test_utils::init_resources;

    #[test]
    fn profiles_fields_validity() {
        init_resources();

        let mut unique = HashSet::new();

        for profile in Profile::all().unwrap() {
            assert!(!profile.id().is_empty());

            assert!(!profile.name().is_empty());
            assert!(!profile.file_extension().is_empty());
            assert_ne!(
                profile.suggested_max_framerate(),
                gst::Fraction::from_integer(0)
            );

            assert!(
                unique.insert(profile.id().to_string()),
                "Duplicate id `{}`",
                profile.id()
            );
        }
    }

    #[test]
    fn required_elements_of_webm_vp8() {
        init_resources();

        let elements = Profile::from_id("webm-vp8").unwrap().required_elements();
        assert_eq!(
            elements,
            [
                "videoconvert",
                "vp8enc",
                "queue",
                "audioconvert",
                "opusenc",
                "queue",
                "webmmux"
            ]
        );
    }

    #[test]
    fn required_elements_strip_caps() {
        init_resources();

        let elements = Profile::from_id("mp4").unwrap().required_elements();
        assert!(elements.contains(&"capsfilter".to_string()));
        assert!(elements.contains(&"x264enc".to_string()));
        assert!(elements.contains(&"mp4mux".to_string()));
        assert!(elements.iter().all(|e| !e.contains('=')));
    }

    #[test]
    fn only_mp4_and_matroska_use_the_h264_encoder() {
        init_resources();

        let ids = Profile::all()
            .unwrap()
            .iter()
            .filter(|profile| profile.uses_h264_encoder())
            .map(Profile::id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["mp4", "matroska-h264"]);
    }

    #[test]
    fn profiles_without_the_encoder_marker_expand_as_before() {
        init_resources();

        let n_threads = glib::num_processors().min(MAX_THREAD_COUNT).to_string();
        for id in ["webm-vp8", "gif"] {
            let description = &Profile::from_id(id).unwrap().data().videoenc_bin_str;
            for encoder in [H264Encoder::Intel, H264Encoder::Nvidia, H264Encoder::Cpu] {
                assert_eq!(
                    expand(description, encoder),
                    description.replace("${N_THREADS}", &n_threads)
                );
            }
        }
    }

    /// Attaches `profile` with `encoder` to a pipeline of fake elements.
    fn attach_with(profile: &Profile, encoder: H264Encoder) -> gst::Pipeline {
        let pipeline = gst::Pipeline::new();
        let video_src = gst::ElementFactory::make("fakesrc").build().unwrap();
        let audio_src = gst::ElementFactory::make("fakesrc").build().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        pipeline.add_many([&video_src, &audio_src, &sink]).unwrap();

        profile
            .attach_with(&pipeline, &video_src, Some(&audio_src), &sink, encoder)
            .unwrap_or_else(|err| panic!("can't attach `{}` with {encoder:?}: {err:?}", profile.id()));
        assert!(pipeline.find_unlinked_pad(gst::PadDirection::Sink).is_none());
        assert!(pipeline.find_unlinked_pad(gst::PadDirection::Src).is_none());
        pipeline
    }

    fn has_element(pipeline: &gst::Pipeline, factory_name: &str) -> bool {
        pipeline.iterate_recurse().into_iter().any(|element| {
            element
                .unwrap()
                .factory()
                .is_some_and(|factory| factory.name() == factory_name)
        })
    }

    #[test]
    fn h264_profiles_parse_with_each_installed_encoder() {
        init_resources();
        gst::init().unwrap();

        for id in ["mp4", "matroska-h264"] {
            let profile = Profile::from_id(id).unwrap();
            for encoder in [H264Encoder::Intel, H264Encoder::Nvidia, H264Encoder::Cpu] {
                if !encoder.is_installed() {
                    eprintln!("Skipped {encoder:?}: it is not installed");
                    continue;
                }

                let pipeline = attach_with(profile, encoder);
                for element in encoder.required_elements() {
                    assert!(has_element(&pipeline, element), "{id} {encoder:?} {element}");
                }
                assert!(has_element(&pipeline, "h264parse"));
            }
        }
    }

    #[test]
    fn h264_profiles_need_the_cpu_encoder_elements() {
        init_resources();

        let elements = Profile::from_id("matroska-h264")
            .unwrap()
            .required_elements();
        assert_eq!(
            elements,
            [
                "videoconvert",
                "x264enc",
                "capsfilter",
                "queue",
                "h264parse",
                "audioconvert",
                "opusenc",
                "queue",
                "matroskamux"
            ]
        );
    }

    #[test]
    fn profiles_validity() {
        init_resources();
        gst::init().unwrap();
        gstgif::plugin_register_static().unwrap();

        for profile in Profile::all().unwrap() {
            // These profiles are not supported by the CI runner.
            if matches!(profile.id(), "va-h264") {
                continue;
            }

            // FIXME Remove this. This is needed as x264enc is somehow not found.
            if matches!(profile.id(), "mp4" | "matroska-h264") {
                continue;
            }

            let pipeline = gst::Pipeline::new();

            let dummy_video_src = gst::ElementFactory::make("fakesrc").build().unwrap();
            let dummy_sink = gst::ElementFactory::make("fakesink").build().unwrap();
            pipeline.add_many([&dummy_video_src, &dummy_sink]).unwrap();

            let dummy_audio_src = if profile.supports_audio() {
                let dummy_audio_src = gst::ElementFactory::make("fakesrc").build().unwrap();
                pipeline.add(&dummy_audio_src).unwrap();
                Some(dummy_audio_src)
            } else {
                None
            };

            if let Err(err) = profile.attach(
                &pipeline,
                &dummy_video_src,
                dummy_audio_src.as_ref(),
                &dummy_sink,
            ) {
                panic!("can't attach profile `{}`: {:?}", profile.id(), err);
            }

            assert!(
                pipeline
                    .find_unlinked_pad(gst::PadDirection::Sink)
                    .is_none()
            );
            assert!(pipeline.find_unlinked_pad(gst::PadDirection::Src).is_none());

            assert!(profile.is_available());
        }
    }
}

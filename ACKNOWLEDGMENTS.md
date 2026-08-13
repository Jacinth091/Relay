# Acknowledgments

Relay is a project I started while learning more about AI-assisted development workflows, coding-agent orchestration, and how different tools can work together in a practical developer experience.

Along the way, I studied other open-source projects to better understand how these kinds of systems can be designed.

## AIF Handoff

One project that helped me a lot during Relay's early design was [AIF Handoff](https://github.com/lee-to/aif-handoff) by `lee-to`.

As someone still learning this area, seeing a working and much more mature orchestration project helped turn several abstract ideas into something I could actually reason about. It gave me a useful reference for thinking about concepts such as:

- separating providers, runtimes, and transports;
- runtime profiles and capabilities;
- durable task and execution state;
- recovery and rework flows;
- extension boundaries.

Relay is not intended to recreate AIF Handoff.

Instead, I used it as one source of inspiration while shaping Relay around a different workflow I personally wanted: having a normal conversation with an AI for planning, explicitly deciding when the plan is ready, and then handing that approved intent to a coding agent for controlled execution.

Many of Relay's ideas also came from the problems I wanted to solve in my own workflow, including preserving coding-agent usage for implementation, supporting different AI providers, keeping the user in control of important decisions, and making the handoff understandable even when an executor needs clarification.

Thank you to `lee-to` and the contributors to AIF Handoff for making their work available publicly. Being able to study projects like this makes it much easier for beginners like me to learn, experiment, and eventually build ideas of our own.

AIF Handoff and Relay are independent projects and are not affiliated.

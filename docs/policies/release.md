# Release Policy

* Artifact. Release artifact is a Docker Image. We’re not provide any guarantees that raw binary will work expectedly on any system.
* Infrequent releases. Prefer to make releases not frequently, but with significant updates like new functionality or critical bug fixes.
* Release worth updates. New capabilities, critical bug fixes are considered as release worth. Additional test coverage, dependency updates (except ones with critical bug fixes), new non-significant features of implemented capabilities are considered not worth releasing.
* Deprecation. Some old APIs or configs can be marked deprecated and will be removed 2 updates later.


# Backward Compatibility Policy

As long as our final artifact is Docker Image we can consider next updates as backward incompatible:

* Removing deprecated API handlers;
* Requirement of new attachable docker volumes;
* Introduction new mandatory configs or config fields;
* Removing or renaming config fields;
* Make earlier optional config field mandatory;

In the same time next updates we consider backward compatible:

* MSRV update;
* Major dependency updates;
* Introduction of new APIs;
* Introduction of new non mandatory configs;
* Introduction of new non mandatory config fields;
* Make earlier mandatory config field optional;
* Change config field default value (except paths which might/should be mounted);
* etc.

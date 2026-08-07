# Dockerfile
FROM node:24-alpine

# Set the working directory
WORKDIR /app

RUN npm install -g @acala-network/chopsticks@1.5.1

# Expose ports for both instances
EXPOSE 8000 8500 9000 9500

# Default command (this will be overridden by docker-compose)
CMD ["chopsticks", "-c", "/app/config.yml", "-p", "9000"]
